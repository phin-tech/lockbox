//! The kinds of provider Playpen can add to OpenShell: a model service's key, and the host it's sent to.
//!
//! OpenShell knows a provider's type only by a profile imported into its gateway, which names the variable the agent
//! reads the key from, how the key is sent and the hosts it may be sent to. Playpen writes those profiles itself, so
//! one provider serves every sandbox it's attached to, whatever agent runs there. A profile names no programs, so that
//! a new agent never means rewriting a provider: the sandbox's own policy says which program may reach the host, and
//! OpenShell 0.1.8 still puts the key on that program's requests. Only a credential's first variable is set in the
//! sandbox, so each kind has one.

use crate::openshell::Provider;

/// The profile id every LiteLLM proxy's profile starts with; each proxy has its own, naming its host.
const LITELLM_PREFIX: &str = "playpen-litellm-";

/// A kind of provider Playpen knows how to add.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Claude Code on a Claude subscription, with a token from `claude setup-token`.
    ClaudeSubscription,
    Anthropic,
    OpenAi,
    OpenRouter,
    Fireworks,
    ZAi,
    /// A LiteLLM proxy, at an address the person gives.
    LiteLlm,
    /// Cursor's own API key, which only Cursor's agent uses, and only with Cursor's service.
    Cursor,
    /// Codex on a ChatGPT subscription, signed in with a login of the sandboxes' own. OpenShell renews its access
    /// token with the refresh token it holds, so neither ever enters a sandbox.
    ChatGptSubscription,
}

/// Where OpenAI renews a ChatGPT login, and the client Codex signs in as, which a renewal names too.
pub const CHATGPT_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
pub const CHATGPT_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// How the key is sent on each request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Auth {
    /// `Authorization: Bearer <key>`.
    Bearer,
    /// The key alone, in a header of its own.
    Header(&'static str),
}

/// A host and port a provider's key may be sent to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}

/// Why a provider couldn't be added as asked.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    #[error(
        "“{0}” can't name a provider: use lowercase letters, digits and dashes, starting with a letter."
    )]
    Name(String),
    #[error("“{0}” isn't an address Playpen can use: give one like https://litellm.example.com.")]
    Address(String),
    #[error("Paste the key first.")]
    NoKey,
    #[error(
        "“{0}” can't name the key's variable: use capitals, digits and underscores, like GITHUB_TOKEN."
    )]
    Variable(String),
    #[error(
        "1Password's command line, `op`, isn't installed. Install it with `brew install 1password-cli`."
    )]
    NoOp,
    #[error(
        "“{0}” isn't a 1Password secret reference: copy one from 1Password, like op://Private/GitHub/token."
    )]
    Reference(String),
    #[error("Give the 1Password Environment's ID, from Manage environment ▸ Copy environment ID.")]
    Environment,
    #[error("Give the Keychain item's name, as Keychain Access shows it under Where.")]
    Keychain,
}

impl Kind {
    pub const ALL: [Kind; 9] = [
        Kind::ClaudeSubscription,
        Kind::ChatGptSubscription,
        Kind::Anthropic,
        Kind::OpenAi,
        Kind::OpenRouter,
        Kind::Fireworks,
        Kind::ZAi,
        Kind::LiteLlm,
        Kind::Cursor,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::ClaudeSubscription => "Claude Subscription",
            Kind::Anthropic => "Anthropic",
            Kind::OpenAi => "OpenAI",
            Kind::OpenRouter => "OpenRouter",
            Kind::Fireworks => "Fireworks",
            Kind::ZAi => "Z.ai",
            Kind::LiteLlm => "LiteLLM",
            Kind::Cursor => "Cursor",
            Kind::ChatGptSubscription => "ChatGPT Subscription",
        }
    }

    /// The kind of provider OpenShell lists with the type `kind`, if Playpen knows it. `claude-code-oauth` is kept
    /// unprefixed, since providers were made with it before Playpen wrote profiles.
    pub fn of(kind: &str) -> Option<Kind> {
        if kind.starts_with(LITELLM_PREFIX) {
            return Some(Kind::LiteLlm);
        }
        Kind::ALL
            .into_iter()
            .filter(|kind| *kind != Kind::LiteLlm)
            .find(|known| known.fixed_id() == kind)
    }

    /// The id of this kind's profile, for every kind with a fixed host.
    fn fixed_id(self) -> &'static str {
        match self {
            Kind::ClaudeSubscription => "claude-code-oauth",
            Kind::Anthropic => "playpen-anthropic",
            Kind::OpenAi => "playpen-openai",
            Kind::OpenRouter => "playpen-openrouter",
            Kind::Fireworks => "playpen-fireworks",
            Kind::ZAi => "playpen-zai",
            Kind::LiteLlm => "playpen-litellm",
            Kind::Cursor => "playpen-cursor",
            Kind::ChatGptSubscription => "playpen-chatgpt",
        }
    }

    /// The variable the agent finds the key in. OpenShell stores the key under this name too.
    pub fn variable(self) -> &'static str {
        match self {
            Kind::ClaudeSubscription => "CLAUDE_CODE_OAUTH_TOKEN",
            Kind::Anthropic => "ANTHROPIC_API_KEY",
            Kind::OpenAi => "OPENAI_API_KEY",
            Kind::OpenRouter => "OPENROUTER_API_KEY",
            Kind::Fireworks => "FIREWORKS_API_KEY",
            Kind::ZAi => "ZAI_API_KEY",
            Kind::LiteLlm => "LITELLM_API_KEY",
            Kind::Cursor => "CURSOR_API_KEY",
            Kind::ChatGptSubscription => "CHATGPT_ACCESS_TOKEN",
        }
    }

    fn auth(self) -> Auth {
        match self {
            Kind::Anthropic => Auth::Header("x-api-key"),
            _ => Auth::Bearer,
        }
    }

    /// The hosts the key goes to, for every kind but LiteLLM, whose host is the person's.
    pub fn hosts(self) -> &'static [&'static str] {
        match self {
            Kind::ClaudeSubscription | Kind::Anthropic => &["api.anthropic.com"],
            Kind::OpenAi => &["api.openai.com"],
            Kind::OpenRouter => &["openrouter.ai"],
            Kind::Fireworks => &["api.fireworks.ai"],
            Kind::ZAi => &["api.z.ai"],
            Kind::LiteLlm => &[],
            Kind::Cursor => &["api2.cursor.sh"],
            Kind::ChatGptSubscription => &["chatgpt.com", "api.openai.com"],
        }
    }

    /// Whether the person gives its address.
    pub fn needs_address(self) -> bool {
        self.hosts().is_empty()
    }

    /// Whether the person signs in rather than pasting a key.
    pub fn signs_in(self) -> bool {
        self == Kind::ChatGptSubscription
    }

    /// Where the person gets a key, said beside the field it goes in.
    pub fn where_to_get_a_key(self) -> &'static str {
        match self {
            Kind::ClaudeSubscription => {
                "Sign in with Claude to get a token, or run `claude setup-token` and paste the token it prints."
            }
            Kind::Anthropic => "An API key from console.anthropic.com.",
            Kind::OpenAi => "An API key from platform.openai.com.",
            Kind::OpenRouter => "A key from openrouter.ai/keys.",
            Kind::Fireworks => "An API key from fireworks.ai.",
            Kind::ZAi => "An API key from z.ai.",
            Kind::LiteLlm => "A virtual key from the proxy's admin.",
            Kind::Cursor => "A User API Key from cursor.com/dashboard, under Integrations.",
            Kind::ChatGptSubscription => {
                "Sign In… gives a code to sign in to ChatGPT with, in any browser. This is a login of the sandboxes' own, so your Mac's Codex stays signed in."
            }
        }
    }

    /// The name Playpen suggests for a new provider of this kind.
    pub fn suggested_name(self) -> &'static str {
        match self {
            Kind::ClaudeSubscription => "claude-subscription",
            Kind::Anthropic => "anthropic",
            Kind::OpenAi => "openai",
            Kind::OpenRouter => "openrouter",
            Kind::Fireworks => "fireworks",
            Kind::ZAi => "zai",
            Kind::LiteLlm => "litellm",
            Kind::Cursor => "cursor",
            Kind::ChatGptSubscription => "chatgpt",
        }
    }
}

impl Endpoint {
    /// The host and port in an address such as `https://litellm.example.com` or `http://10.0.0.5:4000`. A bare host
    /// is taken as HTTPS.
    pub fn from_address(address: &str) -> Result<Endpoint, ProviderError> {
        let bad = || ProviderError::Address(address.to_string());
        let trimmed = address.trim();
        let (default_port, rest) = if let Some(rest) = trimmed.strip_prefix("https://") {
            (443, rest)
        } else if let Some(rest) = trimmed.strip_prefix("http://") {
            (80, rest)
        } else if trimmed.contains("://") {
            return Err(bad());
        } else {
            (443, trimmed)
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if authority.contains('@') {
            return Err(bad());
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host, port.parse::<u16>().map_err(|_| bad())?),
            None => (authority, default_port),
        };
        let host = host.to_ascii_lowercase();
        let valid = !host.is_empty()
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
            && !host.starts_with(['.', '-'])
            && port != 0;
        if valid {
            Ok(Endpoint { host, port })
        } else {
            Err(bad())
        }
    }
}

/// A provider to add: its kind, its name, and, for LiteLLM, where the proxy is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProvider {
    pub kind: Kind,
    pub name: String,
    pub endpoint: Option<Endpoint>,
}

impl NewProvider {
    /// A provider of `kind` called `name`, at `address` when the kind needs one.
    pub fn new(kind: Kind, name: &str, address: &str) -> Result<NewProvider, ProviderError> {
        let name = name.trim().to_string();
        if !crate::is_sandbox_name(&name) {
            return Err(ProviderError::Name(name));
        }
        let endpoint = if kind.needs_address() {
            Some(Endpoint::from_address(address)?)
        } else {
            None
        };
        Ok(NewProvider {
            kind,
            name,
            endpoint,
        })
    }

    /// The id of its profile: the kind's own, or for a LiteLLM proxy one of its own, after the provider's name.
    pub fn profile_id(&self) -> String {
        match self.kind {
            Kind::LiteLlm => format!("{LITELLM_PREFIX}{}", self.name),
            kind => kind.fixed_id().to_string(),
        }
    }

    /// The profile OpenShell takes the provider's type from, as YAML.
    pub fn profile(&self) -> String {
        let kind = self.kind;
        let endpoints: Vec<Endpoint> = match &self.endpoint {
            Some(endpoint) => vec![endpoint.clone()],
            None => kind
                .hosts()
                .iter()
                .map(|host| Endpoint {
                    host: (*host).to_string(),
                    port: 443,
                })
                .collect(),
        };
        let (auth_style, header) = match kind.auth() {
            Auth::Bearer => ("bearer", "authorization"),
            Auth::Header(header) => ("header", header),
        };
        let (display, description) = match (kind, endpoints.first()) {
            (Kind::LiteLlm, Some(endpoint)) => (
                format!("LiteLLM ({})", endpoint.host),
                format!(
                    "The LiteLLM proxy at {}:{}, added by Playpen",
                    endpoint.host, endpoint.port
                ),
            ),
            (kind, _) => (
                kind.name().to_string(),
                format!("{}, added by Playpen", kind.name()),
            ),
        };
        // OpenShell renews a ChatGPT login's access token before it expires, a day ahead, with the refresh token
        // Playpen gives it as secret material.
        let refresh = if kind == Kind::ChatGptSubscription {
            format!(
                "    refresh:\n      \
                   strategy: oauth2_refresh_token\n      \
                   token_url: {CHATGPT_TOKEN_URL}\n      \
                   refresh_before: \"86400s\"\n      \
                   material:\n        \
                     - name: client_id\n          \
                       required: true\n          \
                       secret: false\n        \
                     - name: refresh_token\n          \
                       required: true\n          \
                       secret: true\n"
            )
        } else {
            String::new()
        };
        let mut profile = format!(
            "# Written by Playpen.\n\
             id: {id}\n\
             display_name: {display}\n\
             description: {description}\n\
             category: inference\n\
             inference_capable: true\n\
             credentials:\n  \
               - name: api_key\n    \
                 description: {key_name}\n    \
                 env_vars: [{variable}]\n    \
                 required: true\n    \
                 auth_style: {auth_style}\n    \
                 header_name: {header}\n\
             {refresh}\
             discovery:\n  \
               credentials: [api_key]\n\
             endpoints:\n",
            id = self.profile_id(),
            display = yaml_string(&display),
            description = yaml_string(&description),
            key_name = yaml_string(&format!("{} key", kind.name())),
            variable = kind.variable(),
        );
        for endpoint in endpoints {
            profile.push_str(&format!(
                "  - host: {host}\n    \
                   port: {port}\n    \
                   protocol: rest\n    \
                   access: read-write\n    \
                   enforcement: enforce\n",
                host = endpoint.host,
                port = endpoint.port,
            ));
        }
        profile
    }
}

impl Provider {
    /// Its kind, if Playpen knows it.
    pub fn playpen_kind(&self) -> Option<Kind> {
        Kind::of(&self.kind)
    }

    /// Whether its profile is one Playpen made for it alone, which goes when it does.
    pub(crate) fn has_own_profile(&self) -> bool {
        self.kind.starts_with(LITELLM_PREFIX)
            || self.kind.starts_with(crate::new_service::SERVICE_PREFIX)
    }
}

/// `text` as a double-quoted YAML string.
fn yaml_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{Endpoint, Kind, NewProvider, ProviderError};

    fn endpoint(host: &str, port: u16) -> Endpoint {
        Endpoint {
            host: host.to_string(),
            port,
        }
    }

    #[test]
    fn a_proxys_address_gives_its_host_and_port() {
        let read = |address| Endpoint::from_address(address);
        assert_eq!(
            read("https://LiteLLM.example.com/v1"),
            Ok(endpoint("litellm.example.com", 443))
        );
        assert_eq!(read("http://10.0.0.5:4000"), Ok(endpoint("10.0.0.5", 4000)));
        assert_eq!(
            read("litellm.example.com:8443"),
            Ok(endpoint("litellm.example.com", 8443))
        );
        for bad in [
            "",
            "ftp://x.example.com",
            "https://user:pw@x.example.com",
            "https://x.example.com:99999",
            "https://x y",
        ] {
            assert!(read(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_provider_needs_a_name_openshell_takes_and_litellm_an_address() {
        assert_eq!(
            NewProvider::new(Kind::OpenRouter, "My Key", ""),
            Err(ProviderError::Name("My Key".to_string()))
        );
        assert!(NewProvider::new(Kind::OpenRouter, "openrouter", "").is_ok());
        assert!(matches!(
            NewProvider::new(Kind::LiteLlm, "work", ""),
            Err(ProviderError::Address(_))
        ));
    }

    #[test]
    fn each_litellm_proxy_has_a_profile_of_its_own_and_others_share_their_kinds() {
        let work = NewProvider::new(Kind::LiteLlm, "work", "https://litellm.example.com").unwrap();
        assert_eq!(work.profile_id(), "playpen-litellm-work");
        assert_eq!(Kind::of("playpen-litellm-work"), Some(Kind::LiteLlm));
        let openrouter = NewProvider::new(Kind::OpenRouter, "mine", "").unwrap();
        assert_eq!(openrouter.profile_id(), "playpen-openrouter");
        assert_eq!(Kind::of("playpen-openrouter"), Some(Kind::OpenRouter));
        assert_eq!(
            Kind::of("claude-code-oauth"),
            Some(Kind::ClaudeSubscription)
        );
        assert_eq!(Kind::of("github"), None);
    }

    #[test]
    fn a_litellm_proxys_profile_sends_its_key_only_to_the_proxy() {
        let work =
            NewProvider::new(Kind::LiteLlm, "work", "http://litellm.example.com:4000").unwrap();
        insta::assert_snapshot!(work.profile());
    }

    #[test]
    fn a_chatgpt_login_is_renewed_by_openshell_and_sent_to_chatgpt_and_openai() {
        let chatgpt = NewProvider::new(Kind::ChatGptSubscription, "chatgpt", "").unwrap();
        insta::assert_snapshot!(chatgpt.profile());
    }

    #[test]
    fn anthropics_profile_sends_its_key_in_its_own_header() {
        let anthropic = NewProvider::new(Kind::Anthropic, "anthropic", "").unwrap();
        insta::assert_snapshot!(anthropic.profile());
    }
}
