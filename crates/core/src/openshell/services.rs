//! Services: providers other than a model's key, such as a GitHub token from 1Password, attached to a sandbox so
//! its agent can use them. OpenShell puts a stand-in for the key in the sandbox and swaps in the key on requests to
//! the provider's hosts, but lets through only programs a policy names, and a profile Playpen didn't write may name
//! none. So Playpen adds a rule of its own for each host, open to any program in the sandbox, with the access the
//! profile gives, never more.

use serde::Deserialize;

use super::{OpenShell, OpenShellError, Provider};

/// A host a profile sends its key to, with the access it allows there.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProfileEndpoint {
    pub host: String,
    pub port: u16,
    #[serde(default = "rest")]
    pub protocol: String,
    #[serde(default = "read_only")]
    pub access: String,
    #[serde(default = "enforce")]
    pub enforcement: String,
}

fn rest() -> String {
    "rest".to_string()
}

fn read_only() -> String {
    "read-only".to_string()
}

fn enforce() -> String {
    "enforce".to_string()
}

/// A provider profile on the gateway: what a provider of its type is, and where its key goes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Profile {
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub inference_capable: bool,
    #[serde(default)]
    pub endpoints: Vec<ProfileEndpoint>,
    #[serde(default)]
    pub credentials: Vec<ProfileCredential>,
}

/// A credential a profile takes, by the variables it's set in; only the first is set in a sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProfileCredential {
    #[serde(default)]
    pub env_vars: Vec<String>,
}

impl Profile {
    /// Whether it's for reaching a model, which an agent reaches only from its own program, so it's never a service.
    pub fn is_for_a_model(&self) -> bool {
        self.inference_capable || matches!(self.category.as_str(), "inference" | "agent")
    }

    /// Its name as people know it, or its id when it has none.
    pub fn name(&self) -> &str {
        if self.display_name.is_empty() {
            &self.id
        } else {
            &self.display_name
        }
    }
}

/// A provider to attach to a sandbox as a service, with the hosts its profile sends its key to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub provider: String,
    pub endpoints: Vec<ProfileEndpoint>,
}

impl Service {
    /// The providers that can be services, with their profiles: those whose profile isn't for a model and sends its
    /// key somewhere.
    pub fn choices<'a>(
        providers: &'a [Provider],
        profiles: &'a [Profile],
    ) -> Vec<(&'a Provider, &'a Profile)> {
        providers
            .iter()
            .filter_map(|provider| {
                let profile = profiles
                    .iter()
                    .find(|profile| profile.id == provider.kind)?;
                (!profile.is_for_a_model() && !profile.endpoints.is_empty())
                    .then_some((provider, profile))
            })
            .collect()
    }

    pub fn new(provider: &Provider, profile: &Profile) -> Service {
        Service {
            provider: provider.name.clone(),
            endpoints: profile.endpoints.clone(),
        }
    }

    /// The name of Playpen's rule for one of its hosts.
    fn rule(&self, endpoint: &ProfileEndpoint) -> String {
        let mut rule = format!(
            "playpen-service-{}-{}-{}",
            self.provider, endpoint.host, endpoint.port
        );
        rule = rule.replace(|c: char| !(c.is_ascii_alphanumeric() || c == '-'), "-");
        rule
    }

    /// Its rules, to go in a sandbox's policy when it's made: one for each host, open to any program.
    pub fn policy(&self) -> String {
        let mut policy = String::new();
        for endpoint in &self.endpoints {
            policy.push_str(&format!(
                "  {rule}:\n    endpoints:\n      - host: {host}\n        port: {port}\n        protocol: {protocol}\n        access: {access}\n        enforcement: {enforcement}\n    binaries:\n      - path: \"/**\"\n",
                rule = self.rule(endpoint),
                host = endpoint.host,
                port = endpoint.port,
                protocol = endpoint.protocol,
                access = endpoint.access,
                enforcement = endpoint.enforcement,
            ));
        }
        policy
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ProfileListing {
    Bare(Vec<Profile>),
    Paged { profiles: Vec<Profile> },
}

/// The profiles in what `profile list -o json` printed, which OpenShell 0.1.8 gives as a bare list.
pub(super) fn parse_profiles(json: &str) -> Result<Vec<Profile>, OpenShellError> {
    match serde_json::from_str(json) {
        Ok(ProfileListing::Bare(profiles) | ProfileListing::Paged { profiles }) => Ok(profiles),
        Err(error) => Err(OpenShellError::Unreadable(error.to_string())),
    }
}

#[derive(Deserialize)]
struct Attached {
    providers: Vec<Provider>,
}

impl OpenShell {
    /// The provider profiles on the gateway.
    pub fn profiles(&self) -> Result<Vec<Profile>, OpenShellError> {
        parse_profiles(&self.run(
            "list the provider profiles",
            &["profile", "list", "--output", "json"],
        )?)
    }

    /// The providers attached to the sandbox `name`.
    pub fn attached(&self, name: &str) -> Result<Vec<Provider>, OpenShellError> {
        let json = self.run(
            &format!("list the providers of “{name}”"),
            &["sandbox", "provider", "list", name, "--output", "json"],
        )?;
        let attached: Attached = serde_json::from_str(&json)
            .map_err(|error| OpenShellError::Unreadable(error.to_string()))?;
        Ok(attached.providers)
    }

    /// Attaches `service` to the running sandbox `name` and opens its hosts, adding to the sandbox's policy rather
    /// than replacing it, so what the person approved stays. If its hosts can't be opened, it's detached again.
    pub fn attach_service(&self, name: &str, service: &Service) -> Result<(), OpenShellError> {
        let doing = format!("attach “{}” to “{name}”", service.provider);
        self.run(
            &doing,
            &[
                "sandbox",
                "provider",
                "attach",
                name,
                &service.provider,
                "--wait",
            ],
        )?;
        for endpoint in &service.endpoints {
            let address = format!(
                "{}:{}:{}:{}:{}",
                endpoint.host,
                endpoint.port,
                endpoint.access,
                endpoint.protocol,
                endpoint.enforcement
            );
            let rule = service.rule(endpoint);
            if let Err(error) = self.run(
                &doing,
                &[
                    "policy",
                    "update",
                    name,
                    "--add-endpoint",
                    &address,
                    "--binary",
                    "/**",
                    "--rule-name",
                    &rule,
                    "--wait",
                ],
            ) {
                let _ = self.detach_service(name, service);
                return Err(error);
            }
        }
        Ok(())
    }

    /// Closes `service`'s hosts in the sandbox `name` and detaches it, so the stand-in goes from new processes.
    pub fn detach_service(&self, name: &str, service: &Service) -> Result<(), OpenShellError> {
        let doing = format!("detach “{}” from “{name}”", service.provider);
        for endpoint in &service.endpoints {
            // A rule that isn't there, as after a failed attach, is no reason to stop.
            let _ = self.run(
                &doing,
                &[
                    "policy",
                    "update",
                    name,
                    "--remove-rule",
                    &service.rule(endpoint),
                    "--wait",
                ],
            );
        }
        self.run(
            &doing,
            &[
                "sandbox",
                "provider",
                "detach",
                name,
                &service.provider,
                "--wait",
            ],
        )
        .map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::{Profile, Service, parse_profiles};
    use crate::openshell::Provider;

    const PROFILES: &str = r#"[
        {"id": "claude-code-oauth", "display_name": "Claude Code", "category": "agent", "inference_capable": true,
         "endpoints": [{"host": "api.anthropic.com", "port": 443, "protocol": "rest", "access": "read-write", "enforcement": "enforce"}]},
        {"id": "op-github", "display_name": "GitHub", "category": "source_control",
         "endpoints": [{"host": "api.github.com", "port": 443, "protocol": "rest", "access": "read-only", "enforcement": "enforce"}]}
    ]"#;

    fn provider(name: &str, kind: &str) -> Provider {
        Provider {
            name: name.to_string(),
            kind: kind.to_string(),
        }
    }

    #[test]
    fn only_providers_whose_profile_isnt_for_a_model_are_services() {
        let profiles = parse_profiles(PROFILES).unwrap();
        let providers = [
            provider("claude-sub", "claude-code-oauth"),
            provider("github", "op-github"),
            provider("lost", "no-such-profile"),
        ];
        let choices = Service::choices(&providers, &profiles);
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].0.name, "github");
        assert_eq!(choices[0].1.name(), "GitHub");
    }

    #[test]
    fn a_services_rule_keeps_its_profiles_access_and_lets_any_program_use_it() {
        let profiles: Vec<Profile> = parse_profiles(PROFILES).unwrap();
        let github = Service::new(&provider("github", "op-github"), &profiles[1]);
        insta::assert_snapshot!(github.policy());
    }
}
