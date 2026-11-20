//! Herdr, where a sandbox's agent can run in a terminal beside the person's others. herdr-core (`hc`) runs the
//! terminal and keeps it alive; the Herdr app shows any terminal labelled with one of its spaces and tabs, and makes
//! a space or tab for an id it doesn't know. Claude Code reports whether it's working through its terminal, so
//! Herdr knows that even though the agent runs in the sandbox.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::openshell::Sandbox;

/// The label on a sandbox's terminal naming the sandbox, so it's found again rather than opened twice.
pub const SANDBOX_LABEL: &str = "lockbox.sandbox";

/// The Herdr space every sandbox's terminal goes in.
const SPACE: &str = "lockbox";

/// Why Herdr couldn't open a sandbox.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HerdrError {
    #[error("Couldn't run {0}: {1}")]
    Run(String, String),
    #[error("Herdr couldn't {0}: {1}")]
    Failed(String, String),
}

/// herdr-core, and the Herdr app if it's installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Herdr {
    hc: PathBuf,
    app: Option<PathBuf>,
}

#[derive(Deserialize)]
struct Listed {
    #[serde(default)]
    terminals: Vec<Terminal>,
}

#[derive(Deserialize)]
struct Terminal {
    address: String,
    #[serde(default)]
    state: String,
}

impl Herdr {
    /// herdr-core where its installer and Homebrew put it, or none. An app opened from the Finder has no shell's
    /// `PATH`, so it's looked for by its full path first.
    pub fn find() -> Option<Herdr> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let local = |name: &str| home.as_ref().map(|home| home.join(".local/bin").join(name));
        let hc = [
            local("hc"),
            Some("/opt/homebrew/bin/hc".into()),
            Some("/usr/local/bin/hc".into()),
        ]
        .into_iter()
        .flatten()
        .find(|place| place.is_file())?;
        let app = [
            local("herdr-app"),
            home.as_ref()
                .map(|home| home.join("Applications/Herdr.app/Contents/MacOS/Herdr")),
            Some("/Applications/Herdr.app/Contents/MacOS/Herdr".into()),
        ]
        .into_iter()
        .flatten()
        .find(|place| place.is_file());
        Some(Herdr { hc, app })
    }

    /// Shows the sandbox's terminal in Herdr: the one already open, or a new one running `command`, in the
    /// Lockbox space, in a tab named after the sandbox.
    pub fn open(&self, sandbox: &Sandbox, command: &str) -> Result<(), HerdrError> {
        let address = match self.open_terminal(&sandbox.name)? {
            Some(address) => address,
            None => {
                let started = run(
                    &self.hc,
                    "start a terminal",
                    &run_args(sandbox, command, &home()),
                )?;
                let started: Terminal = serde_json::from_str(&started).map_err(|error| {
                    HerdrError::Failed("start a terminal".to_string(), error.to_string())
                })?;
                started.address
            }
        };
        let Some(app) = &self.app else {
            return Ok(());
        };
        // The window may not be open; `open` starts it or brings it forward before it's asked to show the tab.
        let _ = Command::new("/usr/bin/open").args(["-a", "Herdr"]).status();
        let tab = tab_id(&sandbox.name);
        for (doing, command, args) in [
            (
                "name the Lockbox space",
                "space.rename",
                serde_json::json!({"id": SPACE, "name": "Lockbox"}),
            ),
            (
                "name the tab",
                "tab.rename",
                serde_json::json!({"id": tab, "name": sandbox.name}),
            ),
            (
                "show the terminal",
                "pane.focus",
                serde_json::json!({"terminal": address}),
            ),
        ] {
            run(
                app,
                doing,
                &["do".to_string(), command.to_string(), args.to_string()],
            )?;
        }
        Ok(())
    }

    /// The live terminal already open on the sandbox `name`, by its address.
    fn open_terminal(&self, name: &str) -> Result<Option<String>, HerdrError> {
        let listed = run(
            &self.hc,
            "list its terminals",
            &[
                "ls".to_string(),
                "--json".to_string(),
                "--label".to_string(),
                format!("{SANDBOX_LABEL}={name}"),
            ],
        )?;
        let listed: Listed = serde_json::from_str(&listed).map_err(|error| {
            HerdrError::Failed("list its terminals".to_string(), error.to_string())
        })?;
        Ok(listed
            .terminals
            .into_iter()
            .find(|terminal| terminal.state == "live")
            .map(|terminal| terminal.address))
    }
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/".to_string())
}

/// The id of the sandbox's tab in the Lockbox space.
fn tab_id(name: &str) -> String {
    format!("{SPACE}-{name}")
}

/// What follows `hc` to start a terminal running `command` for the sandbox, labelled into the Lockbox space and the
/// sandbox's tab. It's kept, so a terminal lost with herdr-core comes back as a shell rather than vanishing, and has
/// no parent, so it outlives whatever started it.
/// It starts in `home`, which names the space until it's renamed.
pub fn run_args(sandbox: &Sandbox, command: &str, home: &str) -> Vec<String> {
    [
        "run",
        "--bg",
        "--json",
        "--keep",
        "--no-parent",
        "--no-notify",
        "--cwd",
        home,
        "--label",
        &format!("{SANDBOX_LABEL}={}", sandbox.name),
        "--label",
        &format!("herdr.space={SPACE}"),
        "--label",
        &format!("herdr.tab={}", tab_id(&sandbox.name)),
        command,
    ]
    .map(String::from)
    .into()
}

/// Runs `program` with `args`, and gives what it printed, or why it failed while doing `doing`.
fn run(program: &Path, doing: &str, args: &[String]) -> Result<String, HerdrError> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| HerdrError::Run(program.display().to_string(), error.to_string()))?;
    let printed = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        Ok(printed)
    } else {
        let said = String::from_utf8_lossy(&output.stderr);
        let said = if said.trim().is_empty() {
            printed
        } else {
            said.into_owned()
        };
        Err(HerdrError::Failed(
            doing.to_string(),
            said.trim().to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::run_args;
    use crate::openshell::{Phase, Sandbox};

    #[test]
    fn a_sandboxs_terminal_is_labelled_into_the_lockbox_space_and_its_own_tab() {
        let sandbox = Sandbox {
            name: "claude-patchbay".to_string(),
            phase: Phase::Ready,
            created: String::new(),
            agent: Some("claude-code".to_string()),
            folder: None,
            workdir: None,
        };
        let args = run_args(
            &sandbox,
            "openshell sandbox exec --name claude-patchbay --tty -- claude",
            "/Users/sam",
        );
        insta::assert_snapshot!(args.join("\n"));
    }
}
