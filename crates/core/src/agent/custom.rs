//! Agents of the person's own: any harness in an image they give, which Playpen starts with the command they say and
//! lets reach the host its model key is for from the program they name. Playpen keeps them in a file of its own, as it
//! does kits.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::files::write_atomically;
use crate::provider::Kind;

use super::Agent;

/// An agent the person describes.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CustomAgent {
    /// What it's called, which names its sandboxes too: lowercase letters, digits and dashes.
    pub name: String,
    /// The image it runs in, which the gateway pulls: `ghcr.io/me/aider:1.2`.
    pub image: String,
    /// What starts it, run in the sandbox's home.
    pub command: String,
    /// What carries on its last conversation, if it can.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continue_command: Option<String>,
    /// The program that reaches its model, by its real path in the image, which the policy lets out: for a Python
    /// harness, Python itself.
    pub program: String,
    /// The kinds of model key it reads, each from the variable OpenShell sets for that kind.
    #[serde(default, with = "kinds")]
    pub takes: Vec<Kind>,
}

/// Why a custom agent can't be used as it's described.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CustomAgentError {
    #[error(
        "“{0}” can't name an agent: use lowercase letters, digits and dashes, starting with a letter."
    )]
    Name(String),
    #[error("“{0}” is one of Playpen's own agents. Give yours another name.")]
    Taken(String),
    #[error("Say which image {0} runs in, such as ghcr.io/you/agent:1.0.")]
    NoImage(String),
    #[error("Say what command starts {0}.")]
    NoCommand(String),
    #[error(
        "Give the full path of the program {0} reaches its model from, such as /usr/local/bin/python3.12: the policy lets only that program out."
    )]
    Program(String),
    #[error("Choose at least one kind of model key {0} reads.")]
    NoKinds(String),
}

impl CustomAgent {
    /// The kinds of key a custom agent can read: API keys, each in a variable of its own. A subscription's token
    /// needs Claude Code or Codex's own sign-in, which only they have.
    pub const KINDS: [Kind; 6] = [
        Kind::OpenRouter,
        Kind::Anthropic,
        Kind::OpenAi,
        Kind::LiteLlm,
        Kind::Fireworks,
        Kind::ZAi,
    ];

    /// Whether it can be used as it is, saying what's missing if not.
    pub fn check(&self) -> Result<(), CustomAgentError> {
        let name = &self.name;
        if !super::is_sandbox_name(name) {
            return Err(CustomAgentError::Name(name.clone()));
        }
        if Agent::ALL.iter().any(|agent| agent.id() == name) || name == super::CUSTOM_LABEL {
            return Err(CustomAgentError::Taken(name.clone()));
        }
        if self.image.trim().is_empty() {
            return Err(CustomAgentError::NoImage(name.clone()));
        }
        if self.command.trim().is_empty() {
            return Err(CustomAgentError::NoCommand(name.clone()));
        }
        if !self.program.starts_with('/') || self.program.contains(char::is_whitespace) {
            return Err(CustomAgentError::Program(name.clone()));
        }
        if self.takes.is_empty() {
            return Err(CustomAgentError::NoKinds(name.clone()));
        }
        Ok(())
    }

    /// Whether a provider OpenShell lists with the type `kind` holds a key it reads.
    pub fn takes(&self, kind: &str) -> bool {
        Kind::of(kind).is_some_and(|known| self.takes.contains(&known))
    }
}

/// Every custom agent, in the order they were made.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CustomAgents {
    #[serde(default, rename = "agent")]
    pub agents: Vec<CustomAgent>,
}

impl CustomAgents {
    /// The agents kept at `path`, or none when there's no file yet. A file that can't be read is said, not lost.
    pub fn load(path: &Path) -> Result<CustomAgents, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|error| error.to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(CustomAgents::default())
            }
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

    pub fn find(&self, name: &str) -> Option<&CustomAgent> {
        self.agents.iter().find(|agent| agent.name == name)
    }

    /// A name no agent has yet: `agent`, then `agent-2` and on.
    pub fn unused_name(&self, base: &str) -> String {
        (1..)
            .map(|n| match n {
                1 => base.to_string(),
                n => format!("{base}-{n}"),
            })
            .find(|name| self.find(name).is_none())
            .unwrap_or_else(|| base.to_string())
    }
}

/// Kinds kept by the name Playpen shows them by, which a person editing the file reads.
mod kinds {
    use serde::{Deserialize as _, Deserializer, Serializer};

    use crate::provider::Kind;

    pub fn serialize<S: Serializer>(kinds: &[Kind], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(kinds.iter().map(|kind| kind.name()))
    }

    /// A name this version doesn't know is left out rather than losing the whole file.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Kind>, D::Error> {
        let names = Vec::<String>::deserialize(deserializer)?;
        Ok(names
            .iter()
            .filter_map(|name| Kind::ALL.into_iter().find(|kind| kind.name() == name))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::{CustomAgent, CustomAgentError, CustomAgents};
    use crate::provider::Kind;

    fn aider() -> CustomAgent {
        CustomAgent {
            name: "aider".to_string(),
            image: "ghcr.io/me/aider:1.2".to_string(),
            command: "aider".to_string(),
            continue_command: Some("aider --restore-chat-history".to_string()),
            program: "/usr/local/bin/python3.12".to_string(),
            takes: vec![Kind::OpenRouter, Kind::LiteLlm],
        }
    }

    #[test]
    fn custom_agents_are_kept_with_their_keys_by_name() {
        let agents = CustomAgents {
            agents: vec![aider()],
        };
        let text = toml::to_string_pretty(&agents).unwrap();
        insta::assert_snapshot!(text);
        assert_eq!(toml::from_str::<CustomAgents>(&text).unwrap(), agents);
    }

    #[test]
    fn a_custom_agent_needs_a_free_name_an_image_a_command_a_program_and_a_key() {
        assert_eq!(aider().check(), Ok(()));
        let named = |name: &str| CustomAgent {
            name: name.to_string(),
            ..aider()
        };
        assert_eq!(
            named("pi").check(),
            Err(CustomAgentError::Taken("pi".to_string()))
        );
        assert_eq!(
            named("custom").check(),
            Err(CustomAgentError::Taken("custom".to_string()))
        );
        assert!(matches!(
            named("Aider").check(),
            Err(CustomAgentError::Name(_))
        ));
        let relative = CustomAgent {
            program: "python3".to_string(),
            ..aider()
        };
        assert!(matches!(
            relative.check(),
            Err(CustomAgentError::Program(_))
        ));
        let keyless = CustomAgent {
            takes: Vec::new(),
            ..aider()
        };
        assert!(matches!(keyless.check(), Err(CustomAgentError::NoKinds(_))));
    }

    #[test]
    fn a_custom_agent_takes_only_the_kinds_of_key_it_reads() {
        assert!(aider().takes("playpen-openrouter"));
        assert!(aider().takes("playpen-litellm-work"));
        assert!(!aider().takes("playpen-anthropic"));
        assert!(!aider().takes("claude-code-oauth"));
    }
}
