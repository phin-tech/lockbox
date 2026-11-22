//! A service Playpen adds to OpenShell: a key for something other than a model, such as GitHub, sent only to the one
//! host the person names, read-only unless they say otherwise. Its key can be pasted, or come from 1Password through
//! `op`, which hands it straight to `openshell` so it never passes through Playpen.

use std::path::{Path, PathBuf};

use crate::openshell::Key;
use crate::provider::{Endpoint, ProviderError};
use crate::secrets::{Secret, Wrapped, wrap};

/// The profile id every service's profile starts with; each service has its own, naming its host.
pub const SERVICE_PREFIX: &str = "playpen-service-";

/// What a service lets programs in a sandbox do at its host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    ReadOnly,
    ReadWrite,
}

impl Access {
    fn word(self) -> &'static str {
        match self {
            Access::ReadOnly => "read-only",
            Access::ReadWrite => "read-write",
        }
    }
}

/// Where a service's key comes from.
#[derive(Debug)]
pub enum KeySource {
    /// Pasted.
    Key(Key),
    /// A 1Password secret reference, `op://vault/item/field`, which `op run` reads.
    Reference(String),
    /// A 1Password Environment, by its id, holding a secret named as the service's variable.
    Environment(String),
    /// A Keychain item, by its service name, which `security` reads.
    Keychain(String),
}

/// A service to add: its name, the host its key goes to, the variable its key is in, and what it allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewService {
    pub name: String,
    pub endpoint: Endpoint,
    pub variable: String,
    pub access: Access,
}

/// Whether `name` can name an environment variable: capitals, digits and underscores, not starting with a digit.
pub(crate) fn is_variable(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

impl NewService {
    pub fn new(
        name: &str,
        address: &str,
        variable: &str,
        access: Access,
    ) -> Result<NewService, ProviderError> {
        let name = name.trim().to_string();
        if !crate::is_sandbox_name(&name) {
            return Err(ProviderError::Name(name));
        }
        let variable = variable.trim().to_string();
        if !is_variable(&variable) {
            return Err(ProviderError::Variable(variable));
        }
        Ok(NewService {
            name,
            endpoint: Endpoint::from_address(address)?,
            variable,
            access,
        })
    }

    pub fn profile_id(&self) -> String {
        format!("{SERVICE_PREFIX}{}", self.name)
    }

    /// Its profile, as YAML: the key in `variable`, as a bearer token, to the one host. OpenShell has no category for
    /// services, so it's `other`; Playpen knows its own by the id.
    pub fn profile(&self) -> String {
        let Endpoint { host, port } = &self.endpoint;
        format!(
            "# Written by Playpen.\n\
             id: {id}\n\
             display_name: {display}\n\
             description: {description}\n\
             category: other\n\
             credentials:\n\
             \x20 - name: key\n\
             \x20   description: {key}\n\
             \x20   env_vars: [{variable}]\n\
             \x20   required: true\n\
             \x20   auth_style: bearer\n\
             \x20   header_name: authorization\n\
             endpoints:\n\
             \x20 - host: {host}\n\
             \x20   port: {port}\n\
             \x20   protocol: rest\n\
             \x20   access: {access}\n\
             \x20   enforcement: enforce\n",
            id = self.profile_id(),
            display = yaml_string(&format!("{} ({host})", self.name)),
            description = yaml_string(&format!("A service at {host}:{port}, added by Playpen")),
            key = yaml_string(&format!("{} key", self.name)),
            variable = self.variable,
            access = self.access.word(),
        )
    }
}

/// `text` as a double-quoted YAML string.
fn yaml_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

/// The `op` on this Mac, where Homebrew and 1Password's installer put it.
pub fn find_op() -> Option<PathBuf> {
    ["/opt/homebrew/bin/op", "/usr/local/bin/op"]
        .into_iter()
        .map(PathBuf::from)
        .find(|place| place.is_file())
}

/// The command that runs `openshell` with `args` and the key from `source` in `variable`: set by Playpen when it's
/// pasted, or fetched from 1Password or the Keychain around it, so Playpen never reads it.
pub(crate) fn with_key(
    source: &KeySource,
    variable: &str,
    op: Option<&Path>,
    openshell: &Path,
    args: &[&str],
) -> Result<Wrapped, ProviderError> {
    let mut command = vec![openshell.to_string_lossy().into_owned()];
    command.extend(args.iter().map(|arg| (*arg).to_string()));
    let secret = |secret: Secret| vec![(variable.to_string(), secret)];
    match source {
        KeySource::Key(key) => {
            if key.is_empty() {
                return Err(ProviderError::NoKey);
            }
            let mut wrapped = wrap(command, &[], None, op)?;
            wrapped.env.push((variable.to_string(), key.0.clone()));
            Ok(wrapped)
        }
        KeySource::Reference(reference) => wrap(
            command,
            &secret(Secret::Reference(reference.clone())),
            None,
            op,
        ),
        KeySource::Keychain(service) => wrap(
            command,
            &secret(Secret::Keychain(service.clone())),
            None,
            op,
        ),
        KeySource::Environment(environment) => wrap(command, &[], Some(environment), op),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Access, KeySource, NewService, with_key};
    use crate::provider::ProviderError;

    fn github() -> NewService {
        NewService::new("github", "api.github.com", "GITHUB_TOKEN", Access::ReadOnly).unwrap()
    }

    #[test]
    fn a_services_profile_sends_its_key_to_its_one_host_with_the_access_given() {
        insta::assert_snapshot!(github().profile());
    }

    #[test]
    fn a_services_variable_and_name_are_checked() {
        assert_eq!(
            NewService::new("github", "api.github.com", "github token", Access::ReadOnly),
            Err(ProviderError::Variable("github token".to_string()))
        );
        assert!(matches!(
            NewService::new("GitHub", "api.github.com", "GITHUB_TOKEN", Access::ReadOnly),
            Err(ProviderError::Name(_))
        ));
    }

    #[test]
    fn a_1password_key_reaches_openshell_through_op_run_and_never_playpen() {
        let op = Path::new("/opt/homebrew/bin/op");
        let openshell = Path::new("/opt/homebrew/bin/openshell");
        let args = ["provider", "create", "--credential", "GITHUB_TOKEN"];
        let wrapped = with_key(
            &KeySource::Reference("op://Private/GitHub/token".to_string()),
            "GITHUB_TOKEN",
            Some(op),
            openshell,
            &args,
        )
        .unwrap();
        assert_eq!(wrapped.program, op);
        assert_eq!(wrapped.args[..2], ["run", "--"]);
        assert_eq!(
            wrapped.env,
            [(
                "GITHUB_TOKEN".to_string(),
                "op://Private/GitHub/token".to_string()
            )],
            "op swaps the reference for the secret"
        );
        let wrapped = with_key(
            &KeySource::Environment("env-123".to_string()),
            "GITHUB_TOKEN",
            Some(op),
            openshell,
            &args,
        )
        .unwrap();
        assert_eq!(wrapped.args[..4], ["run", "--environment", "env-123", "--"]);
        assert!(wrapped.env.is_empty());
        let wrapped = with_key(
            &KeySource::Keychain("github".to_string()),
            "GITHUB_TOKEN",
            None,
            openshell,
            &args,
        )
        .unwrap();
        assert_eq!(wrapped.program, Path::new("/bin/sh"));
        assert!(wrapped.env.is_empty(), "the shell reads it, not Playpen");
        assert!(matches!(
            with_key(
                &KeySource::Reference("op://x".to_string()),
                "X",
                None,
                openshell,
                &args
            ),
            Err(ProviderError::NoOp)
        ));
    }
}
