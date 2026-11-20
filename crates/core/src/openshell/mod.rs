//! OpenShell, through its `openshell` command: the sandboxes on its gateway, the providers that hold model keys, and
//! starting, stopping, creating and deleting sandboxes.
//!
//! Lockbox asks the command rather than the gateway's gRPC API, since the command already holds the gateway's mTLS
//! certificates and knows which gateway is active. Every call blocks, so the app makes them off its window's thread.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde::Deserialize;

/// The label Lockbox puts on a sandbox it made, naming its [`crate::Agent`].
pub const AGENT_LABEL: &str = "lockbox.agent";

/// The annotation holding the folder on the Mac that was copied into a sandbox.
pub const FOLDER_ANNOTATION: &str = "lockbox.folder";

/// The annotation holding where the agent works in a sandbox: the copied folder, under `/sandbox`.
pub const WORKDIR_ANNOTATION: &str = "lockbox.workdir";

/// Why OpenShell couldn't do what was asked.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpenShellError {
    #[error(
        "OpenShell isn't installed. Install it with `brew install nvidia/openshell/openshell`, then start its gateway with `brew services start openshell`."
    )]
    NotInstalled,
    #[error("Couldn't run OpenShell: {0}")]
    Run(String),
    /// What `openshell` said when it failed, while doing the first part.
    #[error("Couldn't {0}: {1}")]
    Failed(String, String),
    #[error("OpenShell answered in a way Lockbox doesn't understand: {0}")]
    Unreadable(String),
}

/// One sandbox, as OpenShell lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sandbox {
    pub name: String,
    pub phase: Phase,
    /// When it was made, as OpenShell writes it: `2026-11-20 03:08:15`, in UTC.
    pub created: String,
    /// The agent Lockbox made it for, by the label's value; none for a sandbox made some other way.
    pub agent: Option<String>,
    /// The folder on the Mac copied into it.
    pub folder: Option<String>,
    /// Where its agent works in it.
    pub workdir: Option<String>,
}

/// Where a sandbox is in its life. OpenShell may add phases, which are kept by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Provisioning,
    Ready,
    Stopped,
    Suspended,
    /// Its main process has finished.
    Completed,
    Deleting,
    Error,
    Other(String),
}

impl Phase {
    fn from_name(name: &str) -> Phase {
        match name.to_ascii_lowercase().as_str() {
            "provisioning" | "pending" | "starting" => Phase::Provisioning,
            "ready" | "running" => Phase::Ready,
            "stopped" => Phase::Stopped,
            "suspended" => Phase::Suspended,
            "completed" | "succeeded" => Phase::Completed,
            "deleting" | "terminating" => Phase::Deleting,
            "error" | "failed" => Phase::Error,
            _ => Phase::Other(name.to_string()),
        }
    }

    /// Whether its agent can be opened now.
    pub fn is_ready(&self) -> bool {
        *self == Phase::Ready
    }

    /// Whether it can be started: it's stopped, or its main process has finished.
    pub fn can_start(&self) -> bool {
        matches!(self, Phase::Stopped | Phase::Completed)
    }

    /// Whether it failed, which takes a stop before it will start again.
    pub fn needs_restart(&self) -> bool {
        *self == Phase::Error
    }

    pub fn can_stop(&self) -> bool {
        matches!(self, Phase::Ready | Phase::Provisioning)
    }
}

/// A provider: a credential OpenShell keeps, of a type that says what it's for (`claude-code-oauth`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Provider {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Deserialize)]
struct Listed {
    #[serde(default)]
    sandboxes: Vec<ListedSandbox>,
}

#[derive(Deserialize)]
struct ListedSandbox {
    name: String,
    #[serde(default)]
    phase: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    labels: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    annotations: std::collections::BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct ListedProviders {
    #[serde(default)]
    providers: Vec<Provider>,
}

/// The sandboxes in what `openshell sandbox list -o json` printed, by name.
pub fn parse_sandboxes(json: &str) -> Result<Vec<Sandbox>, OpenShellError> {
    let listed: Listed = serde_json::from_str(json)
        .map_err(|error| OpenShellError::Unreadable(error.to_string()))?;
    let mut sandboxes: Vec<Sandbox> = listed
        .sandboxes
        .into_iter()
        .map(|mut listed| Sandbox {
            phase: Phase::from_name(&listed.phase),
            created: listed.created_at,
            agent: listed.labels.remove(AGENT_LABEL),
            folder: listed.annotations.remove(FOLDER_ANNOTATION),
            workdir: listed.annotations.remove(WORKDIR_ANNOTATION),
            name: listed.name,
        })
        .collect();
    sandboxes.sort_by(|one, other| one.name.cmp(&other.name));
    Ok(sandboxes)
}

/// The providers in what `openshell provider list -o json` printed.
pub fn parse_providers(json: &str) -> Result<Vec<Provider>, OpenShellError> {
    let listed: ListedProviders = serde_json::from_str(json)
        .map_err(|error| OpenShellError::Unreadable(error.to_string()))?;
    Ok(listed.providers)
}

/// What `openshell` said on its standard error, without the box it draws around an error.
fn said(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text
        .lines()
        .map(|line| {
            line.trim_start()
                .trim_start_matches("Error:")
                .trim_start_matches(|c: char| c.is_whitespace() || "│╰─▶×┃╭".contains(c))
                .trim()
        })
        .filter(|line| !line.is_empty())
        .collect();
    let joined = lines.join(" ");
    if joined.is_empty() {
        "it gave no reason".to_string()
    } else {
        joined
    }
}

/// The `openshell` command, found where Homebrew and its installer put it. An app opened from the Finder has no
/// shell's `PATH`, so it's looked for by its full path first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenShell {
    binary: PathBuf,
}

impl OpenShell {
    pub fn new(binary: impl Into<PathBuf>) -> OpenShell {
        OpenShell {
            binary: binary.into(),
        }
    }

    /// The installed `openshell`, if there is one.
    pub fn find() -> Result<OpenShell, OpenShellError> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut places = vec![
            PathBuf::from("/opt/homebrew/bin/openshell"),
            PathBuf::from("/usr/local/bin/openshell"),
        ];
        if let Some(home) = home {
            places.push(home.join(".local/bin/openshell"));
        }
        if let Some(path) = std::env::var_os("PATH") {
            places.extend(std::env::split_paths(&path).map(|dir| dir.join("openshell")));
        }
        places
            .into_iter()
            .find(|place| place.is_file())
            .map(OpenShell::new)
            .ok_or(OpenShellError::NotInstalled)
    }

    pub fn binary(&self) -> &Path {
        &self.binary
    }

    /// Runs `openshell` with `args`, and gives what it printed, or why it failed while doing `doing`.
    fn run(&self, doing: &str, args: &[&str]) -> Result<String, OpenShellError> {
        let output = self.output(args)?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(OpenShellError::Failed(
                doing.to_string(),
                said(&output.stderr),
            ))
        }
    }

    fn output(&self, args: &[&str]) -> Result<Output, OpenShellError> {
        Command::new(&self.binary)
            .args(args)
            // Plain text, since its errors are read back and shown.
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .output()
            .map_err(|error| OpenShellError::Run(error.to_string()))
    }

    pub fn sandboxes(&self) -> Result<Vec<Sandbox>, OpenShellError> {
        parse_sandboxes(&self.run(
            "list the sandboxes",
            &["sandbox", "list", "--output", "json"],
        )?)
    }

    pub fn providers(&self) -> Result<Vec<Provider>, OpenShellError> {
        parse_providers(&self.run(
            "list the providers",
            &["provider", "list", "--output", "json"],
        )?)
    }

    pub fn start(&self, name: &str) -> Result<(), OpenShellError> {
        self.run(&format!("start “{name}”"), &["sandbox", "start", name])
            .map(drop)
    }

    pub fn stop(&self, name: &str) -> Result<(), OpenShellError> {
        self.run(&format!("stop “{name}”"), &["sandbox", "stop", name])
            .map(drop)
    }

    /// Starts a sandbox that failed. OpenShell starts only a stopped one, so it's stopped first; its files are kept.
    pub fn restart(&self, name: &str) -> Result<(), OpenShellError> {
        self.stop(name)?;
        self.start(name)
    }

    pub fn delete(&self, name: &str) -> Result<(), OpenShellError> {
        self.run(&format!("delete “{name}”"), &["sandbox", "delete", name])
            .map(drop)
    }

    /// Makes a sandbox with `args` after `sandbox create`, as [`crate::Plan::create_args`] gives them.
    pub fn create(&self, name: &str, args: &[String]) -> Result<(), OpenShellError> {
        let mut all = vec!["sandbox", "create"];
        all.extend(args.iter().map(String::as_str));
        self.run(&format!("make “{name}”"), &all).map(drop)
    }

    /// Copies the folder `local` into the sandbox, under `into`, leaving out what its `.gitignore` does.
    pub fn upload(&self, name: &str, local: &Path, into: &str) -> Result<(), OpenShellError> {
        let local = local.to_string_lossy();
        self.run(
            &format!("copy {local} into “{name}”"),
            &["sandbox", "upload", name, &local, into],
        )
        .map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::{Phase, parse_providers, parse_sandboxes, said};

    /// What OpenShell 0.1.8 listed with one sandbox Lockbox made.
    const SANDBOXES: &str = include_str!("sandboxes.json");
    const PROVIDERS: &str = include_str!("providers.json");

    #[test]
    fn a_sandbox_lockbox_made_says_which_agent_it_runs() {
        let sandboxes = parse_sandboxes(SANDBOXES).unwrap();
        assert_eq!(sandboxes.len(), 1);
        let sandbox = &sandboxes[0];
        assert_eq!(sandbox.name, "lb-idle");
        assert_eq!(sandbox.phase, Phase::Ready);
        assert_eq!(sandbox.agent.as_deref(), Some("claude-code"));
        assert_eq!(sandbox.created, "2026-11-20 03:08:15");
    }

    #[test]
    fn phases_openshell_may_add_are_kept_by_name() {
        let json = r#"{"sandboxes": [
            {"name": "b", "phase": "Stopped"},
            {"name": "a", "phase": "Hibernating"},
            {"name": "c"}
        ]}"#;
        let sandboxes = parse_sandboxes(json).unwrap();
        let phases: Vec<_> = sandboxes
            .iter()
            .map(|s| (s.name.as_str(), &s.phase))
            .collect();
        assert_eq!(
            phases,
            [
                ("a", &Phase::Other("Hibernating".to_string())),
                ("b", &Phase::Stopped),
                ("c", &Phase::Other(String::new())),
            ]
        );
        assert!(Phase::Stopped.can_start() && !Phase::Stopped.can_stop());
    }

    #[test]
    fn providers_are_read_with_their_types() {
        let providers = parse_providers(PROVIDERS).unwrap();
        assert_eq!(providers[0].name, "claude-sub");
        assert_eq!(providers[0].kind, "claude-code-oauth");
    }

    #[test]
    fn an_error_is_read_without_the_box_drawn_around_it() {
        let stderr = "Error:   × failed to read TLS CA from /x/\n  │ mtls/ca.crt\n  ╰─▶ No such file or directory (os error 2)\n";
        assert_eq!(
            said(stderr.as_bytes()),
            "failed to read TLS CA from /x/ mtls/ca.crt No such file or directory (os error 2)"
        );
        assert_eq!(said(b""), "it gave no reason");
    }
}
