//! Secrets Playpen passes on without reading: a 1Password secret reference, which `op run` swaps for the secret, or
//! a Keychain item, which `security` reads in a shell of its own. Either way the command that's given the secret,
//! `openshell`, is started inside the one that fetches it, so the secret never passes through Playpen.

use std::path::{Path, PathBuf};

use crate::provider::ProviderError;

/// Where a secret is, for a command to be given it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Secret {
    /// `op://vault/item/field`.
    Reference(String),
    /// A Keychain item's service name, as Keychain Access shows it under Where for a password.
    Keychain(String),
}

/// A command to run, with what to set in its environment first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wrapped {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Variables Playpen sets: 1Password references, which `op run` swaps for their secrets, and pasted keys.
    pub env: Vec<(String, String)>,
}

/// Where macOS keeps the command that reads the Keychain.
const SECURITY: &str = "/usr/bin/security";

/// Wraps `command` so each variable in `secrets` holds its secret when it runs: 1Password's through `op run`, with
/// `environment`'s secrets too when given, and the Keychain's through a shell that reads them with `security`.
pub fn wrap(
    command: Vec<String>,
    secrets: &[(String, Secret)],
    environment: Option<&str>,
    op: Option<&Path>,
) -> Result<Wrapped, ProviderError> {
    let mut command = command;
    let mut env = Vec::new();
    let keychain: Vec<(&String, &String)> = secrets
        .iter()
        .filter_map(|(variable, secret)| match secret {
            Secret::Keychain(service) => Some((variable, service)),
            Secret::Reference(_) => None,
        })
        .collect();
    if !keychain.is_empty() {
        // Each item's service is an argument, never part of the script, so no name can change what it runs.
        let mut script = String::new();
        let mut services = Vec::new();
        for (variable, service) in keychain {
            if service.trim().is_empty() {
                return Err(ProviderError::Keychain);
            }
            script.push_str(&format!(
                "{variable}=\"$({SECURITY} find-generic-password -s \"$1\" -w)\" || exit 1; export {variable}; shift; "
            ));
            services.push(service.trim().to_string());
        }
        script.push_str("exec \"$@\"");
        let mut shell = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            script,
            "sh".to_string(),
        ];
        shell.extend(services);
        shell.extend(command);
        command = shell;
    }
    let references: Vec<(&String, &String)> = secrets
        .iter()
        .filter_map(|(variable, secret)| match secret {
            Secret::Reference(reference) => Some((variable, reference)),
            Secret::Keychain(_) => None,
        })
        .collect();
    if !references.is_empty() || environment.is_some() {
        let op = op.ok_or(ProviderError::NoOp)?;
        for (variable, reference) in references {
            let reference = reference.trim();
            if !reference.starts_with("op://") {
                return Err(ProviderError::Reference(reference.to_string()));
            }
            env.push((variable.clone(), reference.to_string()));
        }
        let mut run = vec![op.to_string_lossy().into_owned(), "run".to_string()];
        if let Some(environment) = environment {
            if environment.trim().is_empty() {
                return Err(ProviderError::Environment);
            }
            run.extend(["--environment".to_string(), environment.trim().to_string()]);
        }
        run.push("--".to_string());
        run.extend(command);
        command = run;
    }
    let mut command = command.into_iter();
    let program = command.next().map(PathBuf::from).unwrap_or_default();
    Ok(Wrapped {
        program,
        args: command.collect(),
        env,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Secret, wrap};
    use crate::provider::ProviderError;

    fn openshell() -> Vec<String> {
        [
            "/opt/homebrew/bin/openshell",
            "sandbox",
            "create",
            "--env-from",
            "A",
            "--env-from",
            "B",
        ]
        .map(String::from)
        .to_vec()
    }

    #[test]
    fn a_command_with_no_secrets_runs_as_it_is() {
        let wrapped = wrap(openshell(), &[], None, None).unwrap();
        assert_eq!(wrapped.program, Path::new("/opt/homebrew/bin/openshell"));
        assert!(wrapped.env.is_empty());
    }

    #[test]
    fn one_password_and_the_keychain_fetch_their_secrets_around_the_command() {
        let secrets = [
            (
                "A".to_string(),
                Secret::Reference("op://Work/A/token".to_string()),
            ),
            ("B".to_string(), Secret::Keychain("my service".to_string())),
        ];
        let wrapped = wrap(
            openshell(),
            &secrets,
            None,
            Some(Path::new("/opt/homebrew/bin/op")),
        )
        .unwrap();
        assert_eq!(wrapped.program, Path::new("/opt/homebrew/bin/op"));
        insta::assert_snapshot!(format!("{:#?}", wrapped.args));
        assert_eq!(
            wrapped.env,
            [("A".to_string(), "op://Work/A/token".to_string())]
        );
    }

    #[test]
    fn a_1password_secret_without_op_is_refused() {
        let secrets = [("A".to_string(), Secret::Reference("op://x/y/z".to_string()))];
        assert!(matches!(
            wrap(openshell(), &secrets, None, None),
            Err(ProviderError::NoOp)
        ));
    }
}
