//! Kits: standard sets of keys, such as Work or Personal, each a model key and the services that go with it, by
//! provider name. OpenShell has no such thing, so Playpen keeps them, in a file of its own; they hold names only,
//! since the keys stay with OpenShell.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::files::write_atomically;

/// A set of providers to give a sandbox together.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Kit {
    pub name: String,
    /// The provider holding the model's key, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_key: Option<String>,
    /// The services, by provider name.
    #[serde(default)]
    pub services: Vec<String>,
}

/// Every kit, in the order they were made.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Kits {
    #[serde(default, rename = "kit")]
    pub kits: Vec<Kit>,
}

impl Kits {
    /// The kits kept at `path`, or none when there's no file yet. A file that can't be read is said, not lost.
    pub fn load(path: &Path) -> Result<Kits, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|error| error.to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Kits::default()),
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|error| error.to_string())?;
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
        }
        write_atomically(path, text.as_bytes()).map_err(|error| error.to_string())
    }

    pub fn find(&self, name: &str) -> Option<&Kit> {
        self.kits.iter().find(|kit| kit.name == name)
    }

    /// A name no kit has yet: `kit`, then `kit-2` and on.
    pub fn unused_name(&self, base: &str) -> String {
        (1..)
            .map(|n| match n {
                1 => base.to_string(),
                n => format!("{base}-{n}"),
            })
            .find(|name| self.find(name).is_none())
            .unwrap_or_else(|| base.to_string())
    }

    /// Forgets a provider that's gone, from every kit that had it.
    pub fn forget_provider(&mut self, name: &str) {
        for kit in &mut self.kits {
            if kit.model_key.as_deref() == Some(name) {
                kit.model_key = None;
            }
            kit.services.retain(|service| service != name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Kit, Kits};

    #[test]
    fn kits_are_kept_by_name_and_read_back_as_they_were() {
        let folder = std::env::temp_dir().join(format!("kits-{}", std::process::id()));
        let path = folder.join("kits.toml");
        let _ = std::fs::remove_dir_all(&folder);
        assert_eq!(Kits::load(&path).unwrap(), Kits::default(), "none yet");
        let kits = Kits {
            kits: vec![Kit {
                name: "work".to_string(),
                model_key: Some("claude-work".to_string()),
                services: vec!["github-work".to_string(), "linear".to_string()],
            }],
        };
        kits.save(&path).unwrap();
        insta::assert_snapshot!(std::fs::read_to_string(&path).unwrap());
        assert_eq!(Kits::load(&path).unwrap(), kits);
        assert_eq!(kits.unused_name("work"), "work-2");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_provider_thats_gone_leaves_every_kit() {
        let mut kits = Kits {
            kits: vec![Kit {
                name: "work".to_string(),
                model_key: Some("claude-work".to_string()),
                services: vec!["github".to_string()],
            }],
        };
        kits.forget_provider("claude-work");
        kits.forget_provider("github");
        assert_eq!(kits.kits[0].model_key, None);
        assert!(kits.kits[0].services.is_empty());
    }
}
