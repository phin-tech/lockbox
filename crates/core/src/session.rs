//! Sessions: an agent, or a shell, running in a sandbox in a terminal. A sandbox outlives its sessions. herdr-core
//! keeps a session when it's installed, so closing the window showing it only detaches; without it, the window is the
//! session, and closing it ends the session. Either way, the sandbox is left as it was.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::agent::{open_command, quote};
use crate::herdr::{HerdrApp, HerdrCore, HerdrError};
use crate::openshell::Sandbox;

/// Where sessions are shown unless one is opened somewhere else: kept in the settings.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShowIn {
    /// The Herdr app when it's installed, Terminal otherwise.
    #[default]
    Automatic,
    HerdrApp,
    Ghostty,
    Terminal,
}

/// Where one session is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// A tab in the Herdr app's Playpen space.
    HerdrApp,
    /// A Ghostty window of its own.
    Ghostty,
    /// A Terminal window of its own.
    Terminal,
}

impl ShowIn {
    /// Where a session is shown when nothing else is asked for.
    pub fn place(self, installed: &Installed) -> Place {
        match self {
            ShowIn::Automatic if installed.herdr_app => Place::HerdrApp,
            ShowIn::Automatic | ShowIn::Terminal => Place::Terminal,
            ShowIn::HerdrApp => Place::HerdrApp,
            ShowIn::Ghostty => Place::Ghostty,
        }
    }
}

/// What a session runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum What {
    /// The sandbox's agent, starting a new conversation.
    Agent,
    /// The sandbox's agent, carrying on its last conversation, which the sandbox keeps between sessions.
    Continue,
    /// A shell in the agent's folder, for looking around.
    Shell,
}

impl What {
    /// Its name in a session's label and its tab's id.
    pub fn id(self) -> &'static str {
        match self {
            What::Agent => "agent",
            What::Continue => "continue",
            What::Shell => "shell",
        }
    }
}

/// Which of the programs sessions use are installed, looked for each time, since one may be installed while Playpen
/// runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub herdr_app: bool,
    pub herdr_core: bool,
    pub ghostty: bool,
}

impl Installed {
    pub fn now() -> Installed {
        let herdr_core = HerdrCore::find().is_some();
        Installed {
            herdr_app: herdr_core && HerdrApp::find().is_some(),
            herdr_core,
            ghostty: ghostty().is_some(),
        }
    }

    pub fn has(&self, place: Place) -> bool {
        match place {
            Place::HerdrApp => self.herdr_app,
            Place::Ghostty => self.ghostty,
            Place::Terminal => true,
        }
    }
}

/// Why a session couldn't be opened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Herdr(#[from] HerdrError),
    #[error("Couldn't open {0}: {1}")]
    Window(String, String),
}

/// How a session was opened, when it wasn't quite as asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opened {
    AsAsked,
    /// The place asked for isn't installed, so it opened in Terminal.
    InTerminalInstead(Place),
}

/// Whether herdr-core keeps a session, when it's installed: kept in the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    InHerdrCore,
    /// The window is the session.
    Nowhere,
}

/// Opens the session running `what` in the sandbox, shown in `place`, kept by herdr-core when `keep` asks and it's
/// installed. The Herdr app always has herdr-core keep it, since that's what it shows. A session herdr-core keeps that
/// is already live is shown again rather than started twice.
pub fn open(
    openshell: &Path,
    sandbox: &Sandbox,
    what: What,
    place: Place,
    keep: Keep,
) -> Result<Opened, SessionError> {
    let installed = Installed::now();
    let (place, opened) = if installed.has(place) {
        (place, Opened::AsAsked)
    } else {
        (Place::Terminal, Opened::InTerminalInstead(place))
    };
    let command = open_command(openshell, sandbox, what);
    let core = HerdrCore::find().filter(|_| keep == Keep::InHerdrCore || place == Place::HerdrApp);
    let line = match &core {
        Some(core) => {
            let address = core.session(sandbox, what, &command)?;
            if place == Place::HerdrApp
                && let Some(app) = HerdrApp::find()
            {
                app.show(&sandbox.name, what, &address)?;
                return Ok(opened);
            }
            core.attach_command(&address)
        }
        None => command,
    };
    match place {
        Place::Ghostty => in_ghostty(&line)?,
        Place::Terminal | Place::HerdrApp => {
            in_terminal(&format!("{}-{}", sandbox.name, what.id()), &line)?;
        }
    }
    Ok(opened)
}

/// Ghostty's app, where its installer puts it.
fn ghostty() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    [
        Some(PathBuf::from("/Applications/Ghostty.app")),
        home.map(|home| home.join("Applications/Ghostty.app")),
    ]
    .into_iter()
    .flatten()
    .find(|app| app.is_dir())
}

/// Runs `line` in a new Ghostty. Ghostty runs a command given to it only in the first window a new Ghostty opens, so
/// each session starts one.
fn in_ghostty(line: &str) -> Result<(), SessionError> {
    let failed = |said: String| SessionError::Window("Ghostty".to_string(), said);
    let app = ghostty().ok_or_else(|| failed("it isn't installed".to_string()))?;
    let status = Command::new("/usr/bin/open")
        .arg("-na")
        .arg(&app)
        .args(["--args", "-e", "/bin/sh", "-c", line])
        .status()
        .map_err(|error| failed(error.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        Err(failed(format!("open said {status}")))
    }
}

/// Runs `line` in a new Terminal window of its own, for something the person has to do there, such as signing in.
pub fn show_in_terminal(name: &str, line: &str) -> Result<(), SessionError> {
    in_terminal(name, line)
}

/// Runs `line` in a new Terminal window, as a `.command` script called `name`, which Terminal runs without asking to
/// be controlled.
fn in_terminal(name: &str, line: &str) -> Result<(), SessionError> {
    use std::os::unix::fs::PermissionsExt as _;

    let failed = |said: String| SessionError::Window("Terminal".to_string(), said);
    let folder = std::env::temp_dir().join("playpen");
    std::fs::create_dir_all(&folder).map_err(|error| failed(error.to_string()))?;
    let path = folder.join(format!("{name}.command"));
    std::fs::write(&path, script(line)).map_err(|error| failed(error.to_string()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .map_err(|error| failed(error.to_string()))?;
    let status = Command::new("/usr/bin/open")
        .args(["-a", "Terminal"])
        .arg(&path)
        .status()
        .map_err(|error| failed(error.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        Err(failed(format!("open said {status}")))
    }
}

/// `line` as the script Terminal runs.
pub fn script(line: &str) -> String {
    format!(
        "#!/bin/sh\n# Written by Playpen.\nexec /bin/sh -c {}\n",
        quote(line)
    )
}

#[cfg(test)]
mod tests {
    use super::{Installed, Place, ShowIn, script};

    fn installed(herdr_app: bool) -> Installed {
        Installed {
            herdr_app,
            herdr_core: herdr_app,
            ghostty: false,
        }
    }

    #[test]
    fn automatic_shows_sessions_in_the_herdr_app_only_when_its_installed() {
        assert_eq!(ShowIn::Automatic.place(&installed(true)), Place::HerdrApp);
        assert_eq!(ShowIn::Automatic.place(&installed(false)), Place::Terminal);
        assert_eq!(ShowIn::Terminal.place(&installed(true)), Place::Terminal);
        assert_eq!(ShowIn::Ghostty.place(&installed(false)), Place::Ghostty);
        assert!(
            !installed(false).has(Place::Ghostty),
            "and so opens in Terminal instead"
        );
    }

    #[test]
    fn terminals_script_runs_the_line_as_one_word() {
        insta::assert_snapshot!(script("'/x/hc' attach 't4r8'"));
    }
}
