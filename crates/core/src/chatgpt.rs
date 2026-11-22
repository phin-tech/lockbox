//! Signing sandboxes in to ChatGPT, for Codex on a ChatGPT subscription.
//!
//! The Mac's own Codex signs in, but into a folder of Playpen's rather than `~/.codex`, so the sandboxes have a
//! login of their own: OpenAI replaces a refresh token each time it's used, and sharing one with the Mac's Codex
//! would sign one or the other out. Playpen gives the access and refresh tokens to OpenShell, which renews the login
//! and puts the access token on Codex's requests, and then deletes the folder. What stays is the account the login
//! is for, which Codex in a sandbox needs to say which workspace it's using, and isn't a secret.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::files::write_atomically;
use crate::openshell::Key;

/// Why a sign-in couldn't be read, or an account couldn't be kept.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChatGptError {
    #[error(
        "Codex isn't installed on this Mac, and it's what signs in. Install it with `brew install codex`."
    )]
    NoCodex,
    #[error("Couldn't make a folder to sign in to: {0}")]
    Folder(String),
    #[error("Codex signed in, but Playpen couldn't read its login: {0}")]
    Unreadable(String),
    #[error("Codex didn't give a code to sign in with: {0}")]
    Device(String),
    #[error("Couldn't keep the ChatGPT account for “{0}”: {1}")]
    Account(String, String),
}

/// A sign-in under way, in a folder of its own, which goes when it does, with the `codex login` running it.
#[derive(Debug)]
pub struct Login {
    home: PathBuf,
    codex: Option<std::process::Child>,
}

/// Where to sign in, and the one-time code to give there, for a sign-in by device code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    pub url: String,
    pub code: String,
}

/// `text` without the colours a terminal would show.
fn plain(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // An escape runs to its final letter, as `\x1b[94m` does.
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(c);
        }
    }
    plain
}

/// The page and code in what `codex login --device-auth` printed, once it has printed both.
fn device_code(printed: &str) -> Option<DeviceCode> {
    let printed = plain(printed);
    let url = printed
        .split_whitespace()
        .find(|word| word.starts_with("https://"))?
        .to_string();
    let code = printed
        .lines()
        .map(str::trim)
        .find(|line| {
            (7..=12).contains(&line.len())
                && line.contains('-')
                && line
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
        })?
        .to_string();
    Some(DeviceCode { url, code })
}

/// What a finished sign-in gives.
#[derive(Debug)]
pub struct Tokens {
    pub access: Key,
    pub refresh: Key,
    /// When the access token expires, in milliseconds since 1970.
    pub expires_at_ms: i64,
    pub account: Account,
}

/// The ChatGPT account a login is for, kept by Playpen for each provider, since OpenShell doesn't give back what it's
/// given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub account_id: String,
    /// The login's identity token without its signature: who signed in and on what plan, which Codex shows. Unsigned,
    /// it proves nothing to anyone.
    pub id_token: String,
}

#[derive(Deserialize)]
struct AuthFile {
    tokens: AuthTokens,
}

#[derive(Deserialize)]
struct AuthTokens {
    id_token: String,
    access_token: String,
    refresh_token: String,
    account_id: String,
}

/// The `codex` on this Mac, where Homebrew and npm put it.
pub fn find_codex() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    [
        Some(PathBuf::from("/opt/homebrew/bin/codex")),
        Some(PathBuf::from("/usr/local/bin/codex")),
        home.as_ref().map(|home| home.join(".local/bin/codex")),
        home.as_ref().map(|home| home.join(".bun/bin/codex")),
    ]
    .into_iter()
    .flatten()
    .find(|place| place.is_file())
}

impl Login {
    /// A sign-in in a new folder only this user can read.
    pub fn start() -> Result<Login, ChatGptError> {
        use std::os::unix::fs::DirBuilderExt as _;

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let home =
            std::env::temp_dir().join(format!("playpen-chatgpt-{}-{nanos}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&home)
            .map_err(|error| ChatGptError::Folder(error.to_string()))?;
        Ok(Login { home, codex: None })
    }

    /// Starts `codex login --device-auth` into this sign-in's folder, and gives the page to sign in at and the code
    /// to give it once Codex prints them. It goes on waiting for the person until they've signed in or the sign-in is
    /// dropped. No browser or terminal is opened.
    pub fn start_device(&mut self, codex: &Path) -> Result<DeviceCode, ChatGptError> {
        use std::io::Read as _;

        let failed = |error: String| ChatGptError::Device(error);
        let mut child = std::process::Command::new(codex)
            .args(["login", "--device-auth"])
            .env("CODEX_HOME", &self.home)
            .env("NO_COLOR", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| failed(error.to_string()))?;
        let (sender, receiver) = std::sync::mpsc::channel::<String>();
        for stream in [
            child
                .stdout
                .take()
                .map(|out| Box::new(out) as Box<dyn std::io::Read + Send>),
            child
                .stderr
                .take()
                .map(|err| Box::new(err) as Box<dyn std::io::Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let sender = sender.clone();
            // Read to the end, so Codex never blocks on a full pipe while it waits.
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut buffer = [0u8; 1024];
                while let Ok(read) = stream.read(&mut buffer) {
                    if read == 0 {
                        break;
                    }
                    let _ = sender.send(String::from_utf8_lossy(&buffer[..read]).into_owned());
                }
            });
        }
        drop(sender);
        self.codex = Some(child);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut printed = String::new();
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match receiver.recv_timeout(left) {
                Ok(text) => {
                    printed.push_str(&text);
                    if let Some(code) = device_code(&printed) {
                        return Ok(code);
                    }
                }
                Err(_) => break,
            }
        }
        Err(failed(plain(printed.trim())))
    }

    /// The tokens, once the person has signed in, or none while they haven't. A file that isn't JSON yet is one
    /// Codex is still writing.
    pub fn tokens(&self) -> Option<Result<Tokens, ChatGptError>> {
        let text = std::fs::read_to_string(self.home.join("auth.json")).ok()?;
        serde_json::from_str::<serde_json::Value>(&text).ok()?;
        Some(read_tokens(&text))
    }
}

impl Drop for Login {
    fn drop(&mut self) {
        if let Some(mut codex) = self.codex.take() {
            let _ = codex.kill();
            let _ = codex.wait();
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

/// The tokens in a Codex `auth.json`.
fn read_tokens(text: &str) -> Result<Tokens, ChatGptError> {
    let unreadable = |what: &str| ChatGptError::Unreadable(what.to_string());
    let file: AuthFile =
        serde_json::from_str(text).map_err(|_| unreadable("its auth.json isn't as expected"))?;
    let tokens = file.tokens;
    let expires = claims(&tokens.access_token)
        .and_then(|claims| claims.get("exp")?.as_i64())
        .ok_or_else(|| unreadable("its access token has no expiry"))?;
    let payload = tokens
        .id_token
        .split('.')
        .nth(1)
        .ok_or_else(|| unreadable("its identity token isn't a JWT"))?;
    Ok(Tokens {
        access: Key::new(&tokens.access_token),
        refresh: Key::new(&tokens.refresh_token),
        expires_at_ms: expires * 1000,
        account: Account {
            account_id: tokens.account_id,
            // `{"alg":"none","typ":"JWT"}`, so Codex can still read the claims, and a signature of one letter, since
            // that's the form tried in a sandbox.
            id_token: format!("eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.{payload}.x"),
        },
    })
}

/// The claims in a JWT, read without checking its signature.
fn claims(jwt: &str) -> Option<serde_json::Value> {
    let payload = jwt.split('.').nth(1)?;
    serde_json::from_slice(&base64_url(payload)?).ok()
}

/// Decodes unpadded URL-safe base64, as a JWT's parts are.
fn base64_url(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    };
    let mut bytes = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0;
    for c in text.trim_end_matches('=').bytes() {
        buffer = (buffer << 6) | u32::from(value(c)?);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buffer >> bits) as u8);
        }
    }
    Some(bytes)
}

impl Account {
    /// Keeps the account for the provider `name`, in `folder`.
    pub fn save(&self, folder: &Path, name: &str) -> Result<(), ChatGptError> {
        let failed = |error: String| ChatGptError::Account(name.to_string(), error);
        let text = serde_json::to_string_pretty(self).map_err(|error| failed(error.to_string()))?;
        std::fs::create_dir_all(folder).map_err(|error| failed(error.to_string()))?;
        write_atomically(&folder.join(format!("{name}.json")), text.as_bytes())
            .map_err(|error| failed(error.to_string()))
    }

    /// The account kept for the provider `name`, in `folder`.
    pub fn load(folder: &Path, name: &str) -> Result<Account, ChatGptError> {
        let failed = |error: String| ChatGptError::Account(name.to_string(), error);
        let text = std::fs::read_to_string(folder.join(format!("{name}.json")))
            .map_err(|error| failed(error.to_string()))?;
        serde_json::from_str(&text).map_err(|error| failed(error.to_string()))
    }

    /// Forgets the account for the provider `name`, in `folder`.
    pub fn forget(folder: &Path, name: &str) {
        let _ = std::fs::remove_file(folder.join(format!("{name}.json")));
    }
}

#[cfg(test)]
mod tests {
    use super::{base64_url, read_tokens};

    fn jwt(claims: &str) -> String {
        let encode = |bytes: &[u8]| {
            const ALPHABET: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let mut text = String::new();
            for chunk in bytes.chunks(3) {
                let n = chunk
                    .iter()
                    .enumerate()
                    .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
                for i in 0..=chunk.len() {
                    text.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
                }
            }
            text
        };
        format!(
            "{}.{}.signature",
            encode(br#"{"alg":"RS256"}"#),
            encode(claims.as_bytes())
        )
    }

    #[test]
    fn a_codex_login_gives_its_tokens_its_expiry_and_an_unsigned_identity() {
        let access = jwt(r#"{"exp":1800000000}"#);
        let id = jwt(r#"{"email":"sam@example.com"}"#);
        let text = format!(
            r#"{{"auth_mode":"chatgpt","tokens":{{"id_token":"{id}","access_token":"{access}","refresh_token":"rt-1","account_id":"acct-1"}}}}"#
        );
        let tokens = read_tokens(&text).unwrap();
        assert_eq!(tokens.expires_at_ms, 1_800_000_000_000);
        assert_eq!(tokens.account.account_id, "acct-1");
        let unsigned = &tokens.account.id_token;
        assert!(unsigned.ends_with(".x"), "not its signature");
        assert_eq!(unsigned.split('.').nth(1), id.split('.').nth(1));
        assert!(
            format!("{:?}", tokens.refresh).contains('…'),
            "never printed"
        );
    }

    #[test]
    fn the_page_and_code_are_read_from_what_codex_prints() {
        let printed = "Welcome to Codex [v\x1b[90m0.167.0\x1b[0m]\n\nFollow these steps to sign in with ChatGPT using device code authorization:\n\n1. Open this link in your browser and sign in to your account\n   \x1b[94mhttps://auth.openai.com/codex/device\x1b[0m\n\n2. Enter this one-time code \x1b[90m(expires in 15 minutes)\x1b[0m\n   \x1b[94mABCD-EFGHI\x1b[0m\n";
        assert_eq!(
            super::device_code(printed),
            Some(super::DeviceCode {
                url: "https://auth.openai.com/codex/device".to_string(),
                code: "ABCD-EFGHI".to_string(),
            })
        );
        assert_eq!(super::device_code("Welcome to Codex"), None, "not yet");
    }

    #[test]
    fn base64_url_decodes_without_padding() {
        assert_eq!(base64_url("aGk").unwrap(), b"hi");
        assert_eq!(base64_url("aGVsbG8").unwrap(), b"hello");
        assert!(base64_url("a*b").is_none());
    }
}
