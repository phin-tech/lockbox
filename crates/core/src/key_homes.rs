//! Where a provider's key lives besides OpenShell: in 1Password, as an item Playpen made or one the person names, or
//! in the Keychain, as an item Playpen made. OpenShell never gives a key back, so this is what lets one provider go to
//! another gateway, or take a new key on every gateway at once, without the key being pasted again. What's kept here
//! is where the key is, never the key.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::files::write_atomically;
use crate::new_service::KeySource;
use crate::openshell::Key;

/// The Keychain account every item Playpen keeps is under.
const KEYCHAIN_ACCOUNT: &str = "Playpen";

/// Where a provider's key lives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "in")]
pub enum KeyHome {
    /// A 1Password secret reference, `op://vault/item/field`.
    OnePassword { reference: String },
    /// A Keychain item Playpen made, by its service name.
    Keychain { service: String },
}

/// Why a key couldn't be kept or found again.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyHomeError {
    #[error(
        "1Password's command line, `op`, isn't installed. Install it with `brew install 1password-cli`."
    )]
    NoOp,
    #[error("1Password couldn't {0}: {1}")]
    OnePassword(String, String),
    #[error("The Keychain couldn't {0}: {1}")]
    Keychain(String, String),
}

/// A 1Password vault, to save a key in.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Vault {
    pub id: String,
    pub name: String,
}

fn op_output(
    op: &Path,
    doing: &str,
    args: &[&str],
    input: Option<&[u8]>,
) -> Result<String, KeyHomeError> {
    let failed = |error: String| KeyHomeError::OnePassword(doing.to_string(), error);
    let mut child = Command::new(op)
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| failed(error.to_string()))?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin
            .write_all(input)
            .map_err(|error| failed(error.to_string()))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| failed(error.to_string()))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(failed(op_said(&String::from_utf8_lossy(&output.stderr))))
    }
}

/// What `op` said, without the `[ERROR] 2026/11/22 12:45:59` it starts each line with.
fn op_said(stderr: &str) -> String {
    stderr
        .lines()
        .map(|line| {
            let line = line.trim();
            match line.strip_prefix('[').and_then(|rest| rest.split_once(']')) {
                Some((_, rest)) => {
                    let rest = rest.trim_start();
                    // The date and the time.
                    let mut words = rest.splitn(3, ' ');
                    match (words.next(), words.next(), words.next()) {
                        (Some(date), Some(_), Some(said)) if date.contains('/') => said.to_string(),
                        _ => rest.to_string(),
                    }
                }
                None => line.to_string(),
            }
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The vaults 1Password lets the person save in.
pub fn vaults(op: &Path) -> Result<Vec<Vault>, KeyHomeError> {
    let json = op_output(
        op,
        "list your vaults",
        &["vault", "list", "--format", "json"],
        None,
    )?;
    serde_json::from_str(&json).map_err(|error| {
        KeyHomeError::OnePassword("list your vaults".to_string(), error.to_string())
    })
}

#[derive(Deserialize)]
struct Created {
    id: String,
    vault: CreatedVault,
}

#[derive(Deserialize)]
struct CreatedVault {
    id: String,
}

/// Saves `key` in 1Password as an API Credential called `title` in the vault `vault`, and gives the reference to it.
/// The key goes to `op` on its standard input, inside the item's template, never as an argument other programs see.
pub fn save_in_1password(
    op: &Path,
    vault: &str,
    title: &str,
    key: &Key,
) -> Result<String, KeyHomeError> {
    let template = serde_json::json!({
        "title": title,
        "category": "API_CREDENTIAL",
        "fields": [
            {"id": "credential", "type": "CONCEALED", "label": "credential", "value": key.0},
            {"id": "notesPlain", "type": "STRING", "purpose": "NOTES", "label": "notesPlain",
             "value": "Saved by Playpen, which gives it to OpenShell's gateways."},
        ],
    });
    let json = op_output(
        op,
        "save the key",
        &["item", "create", "--vault", vault, "--format", "json", "-"],
        Some(template.to_string().as_bytes()),
    )?;
    let created: Created = serde_json::from_str(&json).map_err(|error| {
        KeyHomeError::OnePassword("save the key".to_string(), error.to_string())
    })?;
    Ok(format!(
        "op://{}/{}/credential",
        created.vault.id, created.id
    ))
}

/// The Keychain item Playpen keeps the provider `provider`'s key in.
pub fn keychain_service(provider: &str) -> String {
    format!("Playpen: {provider}")
}

/// Saves `key` in the login Keychain as `service`, replacing one that's there, readable without asking by Playpen
/// alone.
pub fn save_in_keychain(service: &str, key: &Key) -> Result<(), KeyHomeError> {
    security_framework::passwords::set_generic_password(service, KEYCHAIN_ACCOUNT, key.0.as_bytes())
        .map_err(|error| KeyHomeError::Keychain("save the key".to_string(), error.to_string()))
}

/// Reads the key Playpen kept in the Keychain as `service`.
fn read_keychain(service: &str) -> Result<Key, KeyHomeError> {
    let bytes = security_framework::passwords::get_generic_password(service, KEYCHAIN_ACCOUNT)
        .map_err(|error| KeyHomeError::Keychain("find the key".to_string(), error.to_string()))?;
    Ok(Key::new(&String::from_utf8_lossy(&bytes)))
}

/// Deletes the Keychain item Playpen kept as `service`.
pub fn forget_keychain(service: &str) {
    let _ = security_framework::passwords::delete_generic_password(service, KEYCHAIN_ACCOUNT);
}

impl KeyHome {
    /// Where to take the key from to give it to OpenShell: 1Password's through `op run`, so Playpen never reads it;
    /// the Keychain's read by Playpen, since its item lets Playpen alone read it without asking.
    pub fn source(&self) -> Result<KeySource, KeyHomeError> {
        match self {
            KeyHome::OnePassword { reference } => Ok(KeySource::Reference(reference.clone())),
            KeyHome::Keychain { service } => read_keychain(service).map(KeySource::Key),
        }
    }

    /// Where it is, in words.
    pub fn describe(&self) -> String {
        match self {
            KeyHome::OnePassword { reference } => format!("1Password, {reference}"),
            KeyHome::Keychain { service } => format!("the Keychain, “{service}”"),
        }
    }
}

/// Where each provider's key lives, by the provider's name, which is its name on every gateway it's on.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct KeyHomes {
    #[serde(default)]
    pub homes: BTreeMap<String, KeyHome>,
}

impl KeyHomes {
    pub fn load(path: &Path) -> KeyHomes {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
        }
        write_atomically(path, text.as_bytes()).map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{KeyHome, KeyHomes};

    #[test]
    fn where_each_key_lives_is_kept_but_never_the_key() {
        let mut homes = KeyHomes::default();
        homes.homes.insert(
            "fireworks".to_string(),
            KeyHome::OnePassword {
                reference: "op://vault-id/item-id/credential".to_string(),
            },
        );
        homes.homes.insert(
            "claude-sub".to_string(),
            KeyHome::Keychain {
                service: "Playpen: claude-sub".to_string(),
            },
        );
        insta::assert_snapshot!(serde_json::to_string_pretty(&homes).unwrap());
    }

    #[test]
    fn what_op_says_is_given_without_its_log_prefix() {
        assert_eq!(
            super::op_said(
                "[ERROR] 2026/11/22 12:45:59 authorization prompt dismissed, please try again\n"
            ),
            "authorization prompt dismissed, please try again"
        );
        assert_eq!(super::op_said("plain words\n"), "plain words");
    }
}
