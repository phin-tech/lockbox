//! Getting a Claude subscription's long-lived token from the Mac's Claude Code, as `claude setup-token` gives it, so
//! the person needn't run it in a terminal and paste what it prints.
//!
//! `setup-token` only runs in a terminal, so it's given one through macOS's `script`, as wide as a token is long, so
//! the token is never wrapped. It prints a link to sign in at, and a box for a code in case the page gives one; once
//! the person has signed in it prints the token and ends. Nothing it prints is logged, and the token goes no further
//! than the caller.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::openshell::Key;

/// Where the token starts, and what's in the rest of it.
const TOKEN_PREFIX: &str = "sk-ant-oat";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClaudeTokenError {
    #[error(
        "Claude Code isn't installed on this Mac, and it's what signs in. Install it from claude.com/code."
    )]
    NoClaude,
    #[error("Couldn't start Claude Code's sign-in: {0}")]
    Start(String),
    #[error("Couldn't give Claude Code the code: {0}")]
    Code(String),
    #[error("Claude Code stopped before it gave a token: {0}")]
    Ended(String),
}

/// The `claude` on this Mac, where its installer, Homebrew and npm put it.
pub fn find_claude() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    [
        home.as_ref().map(|home| home.join(".local/bin/claude")),
        home.as_ref().map(|home| home.join(".claude/local/claude")),
        Some(PathBuf::from("/opt/homebrew/bin/claude")),
        Some(PathBuf::from("/usr/local/bin/claude")),
        home.as_ref().map(|home| home.join(".bun/bin/claude")),
    ]
    .into_iter()
    .flatten()
    .find(|place| place.is_file())
}

/// `text` without what a terminal reads as instructions: colours and cursor moves, which run to a letter, and links
/// and titles, which run to a bell or `ESC \`.
fn plain(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            plain.push(c);
            continue;
        }
        match chars.next() {
            Some(']') => {
                while let Some(next) = chars.next() {
                    if next == '\u{7}' {
                        break;
                    }
                    if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            Some('[') => {
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() || matches!(next, '~' | '@') {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    plain
}

/// The page to sign in at, once `setup-token` has printed it.
fn sign_in_link(printed: &str) -> Option<String> {
    let plain = plain(printed);
    plain
        .split(|c: char| c.is_whitespace() || c == '\u{7}')
        .find(|word| {
            word.starts_with("https://")
                && (word.contains("claude.") || word.contains("anthropic."))
        })
        .map(str::to_string)
}

/// The token, once `setup-token` has printed it.
fn token_in(printed: &str) -> Option<Key> {
    let plain = plain(printed);
    let at = plain.find(TOKEN_PREFIX)?;
    let token: String = plain[at..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    // The prefix alone, in what it says before the token, isn't one.
    (token.len() > 40).then(|| Key::new(&token))
}

/// The last few lines it printed, to say why it ended, without anything that could be a token.
fn last_said(printed: &str) -> String {
    let plain = plain(printed);
    let lines: Vec<&str> = plain
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.contains(TOKEN_PREFIX))
        .collect();
    lines[lines.len().saturating_sub(3)..].join(" ")
}

/// `claude setup-token`, running, until it gives a token or is dropped.
pub struct TokenSignIn {
    child: Child,
    stdin: ChildStdin,
    printed: Arc<Mutex<String>>,
}

impl std::fmt::Debug for TokenSignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // What it printed may hold the token.
        f.debug_struct("TokenSignIn").finish_non_exhaustive()
    }
}

impl TokenSignIn {
    /// Starts `claude setup-token` in a terminal of its own. It opens no browser: the person opens the link.
    pub fn start(claude: &Path) -> Result<TokenSignIn, ClaudeTokenError> {
        let failed = |error: std::io::Error| ClaudeTokenError::Start(error.to_string());
        let mut child = Command::new("/usr/bin/script")
            .args(["-q", "/dev/null", "/bin/sh", "-c"])
            .arg("stty cols 4000 rows 50 2>/dev/null; exec \"$0\" setup-token")
            .arg(claude)
            .env("BROWSER", "/usr/bin/true")
            .env("TERM", "xterm-256color")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(failed)?;
        let (Some(stdin), Some(mut stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            return Err(ClaudeTokenError::Start("no terminal".to_string()));
        };
        let printed = Arc::new(Mutex::new(String::new()));
        let reading = Arc::clone(&printed);
        // Read to the end, so Claude Code never blocks on a full pipe while it waits.
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(read) = stdout.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                if let Ok(mut printed) = reading.lock() {
                    printed.push_str(&String::from_utf8_lossy(&buffer[..read]));
                }
            }
        });
        Ok(TokenSignIn {
            child,
            stdin,
            printed,
        })
    }

    fn printed(&self) -> String {
        self.printed
            .lock()
            .map(|printed| printed.clone())
            .unwrap_or_default()
    }

    /// The page to sign in at, once it's printed.
    pub fn link(&self) -> Option<String> {
        sign_in_link(&self.printed())
    }

    /// The token, once the person has signed in.
    pub fn token(&self) -> Option<Key> {
        token_in(&self.printed())
    }

    /// Gives it the code the sign-in page showed, as typing it and pressing Return would.
    pub fn send_code(&mut self, code: &str) -> Result<(), ClaudeTokenError> {
        let code = code.trim();
        self.stdin
            .write_all(format!("{code}\r").as_bytes())
            .and_then(|()| self.stdin.flush())
            .map_err(|error| ClaudeTokenError::Code(error.to_string()))
    }

    /// Why it ended without a token, once it has.
    pub fn ended(&mut self) -> Option<ClaudeTokenError> {
        match self.child.try_wait() {
            Ok(Some(_)) if self.token().is_none() => {
                Some(ClaudeTokenError::Ended(last_said(&self.printed())))
            }
            _ => None,
        }
    }
}

impl Drop for TokenSignIn {
    fn drop(&mut self) {
        // Claude Code first: `script` gives it a session of its own, so it would outlive the terminal for a while.
        let _ = Command::new("/usr/bin/pkill")
            .args(["-KILL", "-P", &self.child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Ok(mut printed) = self.printed.lock() {
            printed.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{last_said, plain, sign_in_link, token_in};

    /// Roughly what `setup-token` prints, with its colours, a link a terminal can open, and a made-up token.
    const PRINTED: &str = "\u{1b}[?25l\u{1b}[38;2;215;119;87mWelcome to Claude Code\u{1b}[39m\r\n\
        Browser didn't open? Use the URL below to sign in (c to copy)\r\n\
        \u{1b}]8;id=1;https://claude.ai/oauth/authorize?abc123\u{7}https://claude.ai/oauth/authorize?abc123\u{1b}]8;;\u{7}\r\n\
        Paste code here if prompted\r\n";

    const TOKEN: &str = "sk-ant-oat01-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-AbCdEfGhIjKlMnOp-AA";

    #[test]
    fn colours_and_links_are_left_out_of_what_it_said() {
        let said = plain(PRINTED);
        assert!(said.starts_with("Welcome to Claude Code"), "{said:?}");
        assert!(!said.contains('\u{1b}'));
        assert_eq!(said.matches("https://").count(), 1);
    }

    #[test]
    fn the_sign_in_link_is_read_once_its_printed() {
        assert_eq!(sign_in_link("Welcome to Claude Code\r\n"), None);
        assert_eq!(
            sign_in_link(PRINTED).as_deref(),
            Some("https://claude.ai/oauth/authorize?abc123")
        );
    }

    #[test]
    fn the_token_is_read_once_its_printed_and_never_said_again() {
        assert!(token_in(PRINTED).is_none());
        assert!(token_in("Paste the sk-ant-oat token you're given").is_none());
        let done = format!(
            "{PRINTED}\u{1b}[32m✓\u{1b}[39m Your OAuth token (valid for 1 year):\r\n\r\n\u{1b}[1m{TOKEN}\u{1b}[22m\r\n\r\nStore this token securely.\r\n"
        );
        assert_eq!(token_in(&done).unwrap().0, TOKEN);
        assert!(!last_said(&done).contains("sk-ant"));
    }
}
