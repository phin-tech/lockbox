//! Reading a `.env` file to make a kit from: each variable becomes a model key or a service. A `.env` says what a
//! key is called but not where it goes, so well-known names come with their host, and the rest are asked about. A
//! value that's a 1Password secret reference, `op://…`, is the reference, never the secret.

use crate::provider::Kind;

/// A line of a `.env` file: a variable and its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvLine {
    pub variable: String,
    pub value: String,
}

impl EnvLine {
    /// Whether its value is a 1Password secret reference rather than a key.
    pub fn is_reference(&self) -> bool {
        self.value.starts_with("op://")
    }
}

/// What a variable is, as far as its name says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Guess {
    /// A model's key, of a kind Playpen adds.
    Model(Kind),
    /// A service's key, sent as a bearer token to this host.
    Service(&'static str),
    /// Nothing Playpen knows; the person says where it goes, or leaves it out.
    Unknown,
}

/// The variables in a `.env` file, in order: `NAME=value`, `export NAME=value`, quoted or not, leaving out blank
/// lines, comments and anything without a value.
pub fn parse(text: &str) -> Vec<EnvLine> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let line = line.strip_prefix("export ").unwrap_or(line);
            let (variable, value) = line.split_once('=')?;
            let variable = variable.trim();
            if variable.is_empty()
                || !variable
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                return None;
            }
            let value = value.trim();
            let value = match value.chars().next() {
                Some(quote @ ('"' | '\'')) => value
                    .strip_prefix(quote)
                    .and_then(|rest| rest.split_once(quote))
                    .map_or(value, |(inner, _)| inner),
                // Unquoted, a ` #` starts a comment.
                _ => value.split(" #").next().unwrap_or(value).trim(),
            };
            (!value.is_empty()).then(|| EnvLine {
                variable: variable.to_string(),
                value: value.to_string(),
            })
        })
        .collect()
}

/// What a variable is, by its name. Services are only those whose API takes the key as a bearer token, since
/// that's how Playpen's services send it.
pub fn guess(variable: &str) -> Guess {
    match variable {
        "CLAUDE_CODE_OAUTH_TOKEN" => Guess::Model(Kind::ClaudeSubscription),
        "ANTHROPIC_API_KEY" => Guess::Model(Kind::Anthropic),
        "OPENAI_API_KEY" => Guess::Model(Kind::OpenAi),
        "OPENROUTER_API_KEY" => Guess::Model(Kind::OpenRouter),
        "FIREWORKS_API_KEY" => Guess::Model(Kind::Fireworks),
        "ZAI_API_KEY" => Guess::Model(Kind::ZAi),
        "CURSOR_API_KEY" => Guess::Model(Kind::Cursor),
        "GITHUB_TOKEN" | "GH_TOKEN" | "GITHUB_PERSONAL_ACCESS_TOKEN" => {
            Guess::Service("api.github.com")
        }
        "GITLAB_TOKEN" => Guess::Service("gitlab.com"),
        "SENTRY_AUTH_TOKEN" => Guess::Service("sentry.io"),
        "STRIPE_SECRET_KEY" | "STRIPE_API_KEY" => Guess::Service("api.stripe.com"),
        "SLACK_BOT_TOKEN" | "SLACK_TOKEN" => Guess::Service("slack.com"),
        "NOTION_TOKEN" | "NOTION_API_KEY" => Guess::Service("api.notion.com"),
        "HF_TOKEN" | "HUGGINGFACE_TOKEN" | "HUGGING_FACE_HUB_TOKEN" => {
            Guess::Service("huggingface.co")
        }
        "VERCEL_TOKEN" => Guess::Service("api.vercel.com"),
        "CLOUDFLARE_API_TOKEN" => Guess::Service("api.cloudflare.com"),
        "NPM_TOKEN" => Guess::Service("registry.npmjs.org"),
        "DIGITALOCEAN_TOKEN" | "DIGITALOCEAN_ACCESS_TOKEN" => {
            Guess::Service("api.digitalocean.com")
        }
        _ => Guess::Unknown,
    }
}

/// Whether a variable's name says it holds a secret.
pub fn looks_secret(variable: &str) -> bool {
    [
        "TOKEN",
        "KEY",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "AUTH",
        "CREDENTIAL",
        "PRIVATE",
    ]
    .iter()
    .any(|word| variable.to_ascii_uppercase().contains(word))
}

/// A provider name made from a variable's: `github` for `GITHUB_TOKEN`, `stripe` for `STRIPE_SECRET_KEY`.
pub fn suggested_name(variable: &str) -> String {
    let mut name = variable.to_ascii_lowercase();
    for ending in [
        "_personal_access_token",
        "_secret_key",
        "_api_token",
        "_access_token",
        "_auth_token",
        "_bot_token",
        "_api_key",
        "_token",
        "_key",
    ] {
        if let Some(stripped) = name.strip_suffix(ending) {
            name = stripped.to_string();
            break;
        }
    }
    let mut cleaned: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    cleaned = cleaned.trim_matches('-').to_string();
    if !cleaned.starts_with(|c: char| c.is_ascii_lowercase()) {
        cleaned = format!("key-{cleaned}");
    }
    cleaned.truncate(crate::agent::SANDBOX_NAME_MAX);
    cleaned.trim_end_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::{EnvLine, Guess, guess, parse, suggested_name};
    use crate::provider::Kind;

    #[test]
    fn a_env_files_variables_are_read_however_theyre_written() {
        let text = r#"
# keys
export GITHUB_TOKEN=ghp_abc
ANTHROPIC_API_KEY="sk-ant-1"
LINEAR_API_KEY='lin_api_2' # a comment
STRIPE_SECRET_KEY=op://Work/Stripe/secret
EMPTY=
not a line
"#;
        let lines = parse(text);
        let pairs: Vec<(&str, &str)> = lines
            .iter()
            .map(|line| (line.variable.as_str(), line.value.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("GITHUB_TOKEN", "ghp_abc"),
                ("ANTHROPIC_API_KEY", "sk-ant-1"),
                ("LINEAR_API_KEY", "lin_api_2"),
                ("STRIPE_SECRET_KEY", "op://Work/Stripe/secret"),
            ]
        );
        assert!(lines[3].is_reference());
        assert!(
            !EnvLine {
                variable: "X".into(),
                value: "y".into()
            }
            .is_reference()
        );
    }

    #[test]
    fn well_known_names_say_what_they_are_and_the_rest_are_asked_about() {
        assert_eq!(guess("ANTHROPIC_API_KEY"), Guess::Model(Kind::Anthropic));
        assert_eq!(guess("GH_TOKEN"), Guess::Service("api.github.com"));
        assert_eq!(
            guess("LINEAR_API_KEY"),
            Guess::Unknown,
            "Linear doesn't take a bearer key"
        );
        assert_eq!(suggested_name("GITHUB_TOKEN"), "github");
        assert_eq!(suggested_name("STRIPE_SECRET_KEY"), "stripe");
        assert_eq!(suggested_name("HUGGING_FACE_HUB_TOKEN"), "hugging-face-hub");
        assert_eq!(suggested_name("1PASS_KEY"), "key-1pass");
    }
}
