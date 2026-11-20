//! Herdr. herdr-core (`hc`) keeps a session's terminal alive whatever shows it, so closing a window only detaches;
//! and since Claude Code reports whether it's working through its terminal, herdr-core knows that even though the
//! agent runs in the sandbox. The Herdr app shows any terminal labelled with one of its spaces and tabs, making a
//! space or tab for an id it doesn't know.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::agent::quote;
use crate::openshell::Sandbox;
use crate::session::What;

/// The label on a session's terminal naming its sandbox.
pub const SANDBOX_LABEL: &str = "lockbox.sandbox";

/// The label naming the sandbox and what the session runs, so a session is found again rather than opened twice.
const SESSION_LABEL: &str = "lockbox.session";

/// The Herdr space every session goes in.
const SPACE: &str = "lockbox";

/// Why Herdr couldn't do what was asked.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HerdrError {
    #[error("Couldn't run {0}: {1}")]
    Run(String, String),
    #[error("Herdr couldn't {0}: {1}")]
    Failed(String, String),
}

/// What an agent in a session is doing, as it tells herdr-core, from least to most pressing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AgentState {
    Idle,
    Working,
    /// Finished while nobody was looking.
    Done,
    /// Waiting for the person: a question, or a permission.
    Blocked,
}

/// herdr-core, through its `hc` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrCore {
    hc: PathBuf,
}

/// The Herdr app, through its `herdr-app` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrApp {
    app: PathBuf,
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
    #[serde(default)]
    labels: BTreeMap<String, String>,
    #[serde(default)]
    status: Option<Status>,
}

/// A terminal's status: just the word in `hc ls`, or the word with the program that reported it in `hc get`.
#[derive(Deserialize)]
#[serde(untagged)]
enum Status {
    Word(String),
    Reported {
        #[serde(default)]
        state: String,
    },
}

impl Status {
    fn word(&self) -> &str {
        match self {
            Status::Word(word) | Status::Reported { state: word } => word,
        }
    }
}

/// The first of `places` that's a file. An app opened from the Finder has no shell's `PATH`, so programs are looked for
/// by their full paths.
fn first_file(places: impl IntoIterator<Item = Option<PathBuf>>) -> Option<PathBuf> {
    places.into_iter().flatten().find(|place| place.is_file())
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

impl HerdrCore {
    /// herdr-core where its installer and Homebrew put it, or none.
    pub fn find() -> Option<HerdrCore> {
        let hc = first_file([
            home().map(|home| home.join(".local/bin/hc")),
            Some("/opt/homebrew/bin/hc".into()),
            Some("/usr/local/bin/hc".into()),
        ])?;
        Some(HerdrCore { hc })
    }

    /// The session running `what` in the sandbox, by its terminal's address: the one already live, or a new one
    /// running `command`.
    pub fn session(
        &self,
        sandbox: &Sandbox,
        what: What,
        command: &str,
    ) -> Result<String, HerdrError> {
        let session = session_id(&sandbox.name, what);
        let listed =
            self.terminals(&["--label".to_string(), format!("{SESSION_LABEL}={session}")])?;
        if let Some(live) = listed.into_iter().find(|terminal| terminal.state == "live") {
            return Ok(live.address);
        }
        let home = home().unwrap_or_else(|| PathBuf::from("/"));
        let started = run(
            &self.hc,
            "start a terminal",
            &run_args(sandbox, what, command, &home.to_string_lossy()),
        )?;
        let started: Terminal = serde_json::from_str(&started).map_err(|error| {
            HerdrError::Failed("start a terminal".to_string(), error.to_string())
        })?;
        Ok(started.address)
    }

    /// The shell line that shows the session at `address` in a terminal window. Closing the window only detaches.
    pub fn attach_command(&self, address: &str) -> String {
        format!(
            "{} attach {}",
            quote(&self.hc.to_string_lossy()),
            quote(address)
        )
    }

    /// What the agent in each sandbox's sessions is doing, by sandbox: the most pressing, when it has several.
    pub fn agent_states(&self) -> Result<BTreeMap<String, AgentState>, HerdrError> {
        Ok(states(self.terminals(&[])?))
    }

    fn terminals(&self, filter: &[String]) -> Result<Vec<Terminal>, HerdrError> {
        let mut args = vec!["ls".to_string(), "--json".to_string()];
        args.extend_from_slice(filter);
        let listed = run(&self.hc, "list its terminals", &args)?;
        let listed: Listed = serde_json::from_str(&listed).map_err(|error| {
            HerdrError::Failed("list its terminals".to_string(), error.to_string())
        })?;
        Ok(listed.terminals)
    }
}

impl HerdrApp {
    /// The Herdr app where its installer puts it, or none.
    pub fn find() -> Option<HerdrApp> {
        let app = first_file([
            home().map(|home| home.join(".local/bin/herdr-app")),
            home().map(|home| home.join("Applications/Herdr.app/Contents/MacOS/Herdr")),
            Some("/Applications/Herdr.app/Contents/MacOS/Herdr".into()),
        ])?;
        Some(HerdrApp { app })
    }

    /// Brings the app forward on the session at `address`, the one running `what` in the sandbox `name`, naming the
    /// space and tab its labels made.
    pub fn show(&self, name: &str, what: What, address: &str) -> Result<(), HerdrError> {
        // The window may not be open; `open` starts it or brings it forward before it's asked to show the tab.
        let _ = Command::new("/usr/bin/open").args(["-a", "Herdr"]).status();
        let tab_name = match what {
            What::Agent => name.to_string(),
            What::Continue => format!("{name} (continued)"),
            What::Shell => format!("{name} (shell)"),
        };
        let session = session_id(name, what);
        for (doing, command, args) in [
            (
                "name the Lockbox space",
                "space.rename",
                serde_json::json!({"id": SPACE, "name": "Lockbox"}),
            ),
            (
                "name the tab",
                "tab.rename",
                serde_json::json!({"id": tab_id(&session), "name": tab_name}),
            ),
            (
                "show the terminal",
                "pane.focus",
                serde_json::json!({"terminal": address}),
            ),
        ] {
            run(
                &self.app,
                doing,
                &["do".to_string(), command.to_string(), args.to_string()],
            )?;
        }
        Ok(())
    }
}

/// What the agent in each sandbox's live sessions is doing, by sandbox: the most pressing, when it has several.
fn states(terminals: Vec<Terminal>) -> BTreeMap<String, AgentState> {
    let mut states = BTreeMap::new();
    for terminal in terminals {
        if terminal.state != "live" {
            continue;
        }
        let (Some(sandbox), Some(status)) = (terminal.labels.get(SANDBOX_LABEL), terminal.status)
        else {
            continue;
        };
        let state = match status.word() {
            "working" => AgentState::Working,
            "blocked" => AgentState::Blocked,
            "done" => AgentState::Done,
            _ => AgentState::Idle,
        };
        let kept = states.entry(sandbox.clone()).or_insert(state);
        *kept = (*kept).max(state);
    }
    states
}

/// The session running `what` in the sandbox `name`: `claude-patchbay.agent`.
fn session_id(name: &str, what: What) -> String {
    format!("{name}.{}", what.id())
}

/// The id of a session's tab in the Lockbox space.
fn tab_id(session: &str) -> String {
    format!("{SPACE}-{session}")
}

/// What follows `hc` to start a terminal running `command`, the session running `what` in the sandbox, labelled into
/// the Lockbox space and a tab of its own, which the Herdr app shows and anything else ignores. It's kept, so a
/// terminal lost with herdr-core comes back as a shell rather than vanishing, and has no parent, so it outlives
/// whatever started it. It starts in `home`.
pub fn run_args(sandbox: &Sandbox, what: What, command: &str, home: &str) -> Vec<String> {
    let session = session_id(&sandbox.name, what);
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
        &format!("{SESSION_LABEL}={session}"),
        "--label",
        &format!("herdr.space={SPACE}"),
        "--label",
        &format!("herdr.tab={}", tab_id(&session)),
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
    use super::{AgentState, Listed, run_args, states};
    use crate::openshell::{Phase, Sandbox};
    use crate::session::What;

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
            What::Agent,
            "openshell sandbox exec --name claude-patchbay --tty -- claude",
            "/Users/sam",
        );
        insta::assert_snapshot!(args.join("\n"));
    }

    #[test]
    fn a_status_is_read_as_hc_ls_and_hc_get_each_give_it() {
        let listed: Listed = serde_json::from_str(
            r#"{"terminals": [
                {"address": "t1", "state": "live", "status": "idle", "labels": {"lockbox.sandbox": "a"}},
                {"address": "t2", "state": "live", "status": {"app": "claude-code", "state": "blocked"},
                 "labels": {"lockbox.sandbox": "a"}},
                {"address": "t3", "state": "live", "status": "working", "labels": {"lockbox.sandbox": "b"}},
                {"address": "t4", "state": "finished", "status": "working", "labels": {"lockbox.sandbox": "c"}},
                {"address": "t5", "state": "live", "status": null, "labels": {}}
            ]}"#,
        )
        .unwrap();
        let states = states(listed.terminals);
        assert_eq!(
            states.get("a"),
            Some(&AgentState::Blocked),
            "the most pressing"
        );
        assert_eq!(states.get("b"), Some(&AgentState::Working));
        assert_eq!(states.get("c"), None, "a finished session says nothing");
    }
}
