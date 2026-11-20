//! The agents Lockbox runs in a sandbox, and making a sandbox for one: its image, the policy that lets it reach its
//! model and nothing else, and the folder it works on.

use std::path::{Path, PathBuf};

use crate::docker::{Docker, DockerError};
use crate::openshell::{
    AGENT_LABEL, FOLDER_ANNOTATION, OpenShell, OpenShellError, Sandbox, WORKDIR_ANNOTATION,
};

/// The Claude Code release the image is built with. Raising it builds a new image the next time one is needed.
const CLAUDE_CODE_VERSION: &str = "2.1.329";

/// Where a sandbox's own files are, and where a folder is copied to.
const SANDBOX_HOME: &str = "/sandbox";

/// An agent Lockbox knows how to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Agent {
    ClaudeCode,
}

impl Agent {
    pub const ALL: [Agent; 1] = [Agent::ClaudeCode];

    /// Its name in a sandbox's label.
    pub fn id(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude-code",
        }
    }

    pub fn from_id(id: &str) -> Option<Agent> {
        Agent::ALL.into_iter().find(|agent| agent.id() == id)
    }

    pub fn name(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "Claude Code",
        }
    }

    /// The image it runs in, tagged with the agent's version.
    pub fn image(self) -> String {
        match self {
            Agent::ClaudeCode => format!("lockbox-claude-code:{CLAUDE_CODE_VERSION}"),
        }
    }

    fn dockerfile(self) -> &'static str {
        match self {
            Agent::ClaudeCode => include_str!("claude-code.Dockerfile"),
        }
    }

    fn build_args(self) -> Vec<(&'static str, &'static str)> {
        match self {
            Agent::ClaudeCode => vec![("CLAUDE_CODE_VERSION", CLAUDE_CODE_VERSION)],
        }
    }

    /// The command that starts it.
    pub fn command(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude",
        }
    }

    /// The program that reaches the model, by its real path in the image. OpenShell matches a rule's binaries by
    /// the path a connection really came from, so `/usr/local/bin/claude`, a link to this, matches nothing.
    fn binary(self) -> &'static str {
        match self {
            Agent::ClaudeCode => {
                "/usr/local/lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe"
            }
        }
    }

    /// The types of provider that hold a key it can use: a subscription's token, or an API key.
    pub fn provider_kinds(self) -> &'static [&'static str] {
        match self {
            Agent::ClaudeCode => &["claude-code-oauth", "claude-code", "anthropic"],
        }
    }

    /// The hosts it needs: its model's API, and, for Claude Code, the service it checks for when it starts
    /// interactively, which it won't start without.
    fn hosts(self) -> &'static [&'static str] {
        match self {
            Agent::ClaudeCode => &["api.anthropic.com", "platform.claude.com"],
        }
    }

    /// What its environment needs in the sandbox: nothing it can do without, since nothing else is reachable. It's
    /// set on the sandbox, since an image's own environment doesn't reach a command OpenShell runs in it.
    pub fn env(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Agent::ClaudeCode => &[
                ("DISABLE_AUTOUPDATER", "1"),
                ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
            ],
        }
    }

    /// The sandbox's policy: the hosts it needs, from its own binary, and nothing else. A provider adds its own
    /// rules, but one written for another image names another path, so the agent's are written here too.
    pub fn policy(self) -> String {
        let mut policy = format!(
            "# Written by Lockbox. Everything this doesn't allow is denied.\n\
             version: 1\n\
             network_policies:\n\
             \x20 {id}:\n\
             \x20   endpoints:\n",
            id = self.id(),
        );
        for host in self.hosts() {
            policy.push_str(&format!(
                "      - host: {host}\n        port: 443\n        protocol: rest\n        access: read-write\n        enforcement: enforce\n"
            ));
        }
        policy.push_str(&format!("    binaries:\n      - path: {}\n", self.binary()));
        policy
    }
}

/// A sandbox to make for an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub name: String,
    pub agent: Agent,
    /// The provider holding its model's key, by name.
    pub provider: Option<String>,
    /// A folder on the Mac to copy in, for it to work on.
    pub folder: Option<PathBuf>,
}

/// How far making a sandbox has got, for showing while it waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Building the agent's image, which happens once for each version.
    Building,
    Creating,
    /// Copying the folder in.
    Copying,
}

/// Why a sandbox couldn't be made.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MakeError {
    #[error(transparent)]
    Docker(#[from] DockerError),
    #[error(transparent)]
    OpenShell(#[from] OpenShellError),
    #[error("Couldn't write the sandbox's policy: {0}")]
    Policy(String),
    #[error(
        "“{0}” can't name a sandbox: use lowercase letters, digits and dashes, starting with a letter."
    )]
    Name(String),
}

impl Plan {
    /// Where the agent works in the sandbox: the copied folder, or the sandbox's home.
    pub fn workdir(&self) -> String {
        match self.folder.as_deref().and_then(Path::file_name) {
            Some(name) => format!("{SANDBOX_HOME}/{}", name.to_string_lossy()),
            None => SANDBOX_HOME.to_string(),
        }
    }

    /// What follows `openshell sandbox create`, with the policy at `policy`. The sandbox starts with no command and
    /// stays up, so its agent can be opened, closed and opened again; the folder is copied in after, since OpenShell
    /// can't copy before a command it's given.
    pub fn create_args(&self, policy: &Path) -> Vec<String> {
        let mut args: Vec<String> = [
            "--name",
            &self.name,
            "--from",
            &self.agent.image(),
            "--policy",
            &policy.to_string_lossy(),
            "--detach",
            // Never a prompt, since nobody sees one, and nothing it wasn't given.
            "--no-auto-providers",
            // Blocked connections become proposals that wait for the person, never rules on their own.
            "--approval-mode",
            "manual",
            "--label",
            &format!("{AGENT_LABEL}={}", self.agent.id()),
            "--annotation",
            &format!("{WORKDIR_ANNOTATION}={}", self.workdir()),
        ]
        .map(String::from)
        .into();
        for (name, value) in self.agent.env() {
            args.extend(["--env".to_string(), format!("{name}={value}")]);
        }
        if let Some(folder) = &self.folder {
            args.extend([
                "--annotation".to_string(),
                format!("{FOLDER_ANNOTATION}={}", folder.display()),
            ]);
        }
        if let Some(provider) = &self.provider {
            args.extend(["--provider".to_string(), provider.clone()]);
        }
        args
    }

    /// Makes the sandbox: builds the agent's image if it isn't yet, creates the sandbox and copies the folder in,
    /// saying each step to `step` as it starts.
    pub fn make(
        &self,
        openshell: &OpenShell,
        docker: &Docker,
        step: impl Fn(Step),
    ) -> Result<(), MakeError> {
        if !is_sandbox_name(&self.name) {
            return Err(MakeError::Name(self.name.clone()));
        }
        let image = self.agent.image();
        if !docker.has_image(&image)? {
            step(Step::Building);
            docker.build(&image, self.agent.dockerfile(), &self.agent.build_args())?;
        }
        step(Step::Creating);
        let policy = std::env::temp_dir().join(format!(
            "lockbox-policy-{}-{}.yaml",
            std::process::id(),
            self.name
        ));
        std::fs::write(&policy, self.agent.policy())
            .map_err(|error| MakeError::Policy(error.to_string()))?;
        let created = openshell.create(&self.name, &self.create_args(&policy));
        let _ = std::fs::remove_file(&policy);
        created?;
        if let Some(folder) = &self.folder {
            step(Step::Copying);
            // A sandbox without its folder isn't what was asked for, and would keep the name from being tried again.
            if let Err(error) = openshell.upload(&self.name, folder, SANDBOX_HOME) {
                let _ = openshell.delete(&self.name);
                return Err(error.into());
            }
        }
        Ok(())
    }
}

/// Whether OpenShell takes `name` for a sandbox: a DNS label, as a container's name is.
pub fn is_sandbox_name(name: &str) -> bool {
    (1..=63).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A sandbox name made from a folder's name, for `agent`: `claude-patchbay` for a folder `Patchbay`.
pub fn suggested_name(agent: Agent, folder: Option<&Path>) -> String {
    let prefix = agent.command();
    let Some(folder) = folder.and_then(Path::file_name) else {
        return prefix.to_string();
    };
    let mut name = format!("{prefix}-");
    for c in folder.to_string_lossy().chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            name.push(c);
        } else if !name.ends_with('-') {
            name.push('-');
        }
    }
    name.truncate(63);
    name.trim_end_matches('-').to_string()
}

/// A shell script that opens the sandbox's agent in its folder, or a shell in a sandbox with no agent Lockbox knows,
/// in a terminal: what Terminal runs for a `.command` file.
pub fn open_script(openshell: &Path, sandbox: &Sandbox) -> String {
    let openshell = quote(&openshell.to_string_lossy());
    let name = quote(&sandbox.name);
    match sandbox.agent.as_deref().and_then(Agent::from_id) {
        Some(agent) => format!(
            "#!/bin/sh\n# Written by Lockbox: opens {agent} in the sandbox “{shown}”.\nexec {openshell} sandbox exec --name {name} --tty --workdir {workdir} -- {command}\n",
            agent = agent.name(),
            shown = sandbox.name,
            workdir = quote(sandbox.workdir.as_deref().unwrap_or(SANDBOX_HOME)),
            command = agent.command(),
        ),
        None => format!(
            "#!/bin/sh\n# Written by Lockbox: opens a shell in the sandbox “{shown}”.\nexec {openshell} sandbox connect {name}\n",
            shown = sandbox.name,
        ),
    }
}

/// `text` as one word to a POSIX shell.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Agent, Plan, is_sandbox_name, open_script, suggested_name};
    use crate::openshell::{Phase, Sandbox};

    fn plan() -> Plan {
        Plan {
            name: "claude-patchbay".to_string(),
            agent: Agent::ClaudeCode,
            provider: Some("claude-sub".to_string()),
            folder: Some(PathBuf::from("/Users/sam/src/Patchbay")),
        }
    }

    #[test]
    fn claude_codes_policy_allows_its_model_from_its_real_binary_and_nothing_else() {
        insta::assert_snapshot!(Agent::ClaudeCode.policy());
    }

    #[test]
    fn a_sandbox_is_made_detached_labelled_and_waiting_for_approval() {
        let args = plan().create_args(Path::new("/tmp/policy.yaml"));
        insta::assert_snapshot!(args.join("\n"));
    }

    #[test]
    fn the_agent_works_in_the_folder_copied_in() {
        assert_eq!(plan().workdir(), "/sandbox/Patchbay");
        let none = Plan {
            folder: None,
            ..plan()
        };
        assert_eq!(none.workdir(), "/sandbox");
    }

    #[test]
    fn a_folders_name_becomes_a_sandboxs() {
        let named = |folder: &str| suggested_name(Agent::ClaudeCode, Some(Path::new(folder)));
        assert_eq!(named("/src/Patchbay"), "claude-patchbay");
        assert_eq!(named("/src/my app (2)"), "claude-my-app-2");
        assert_eq!(suggested_name(Agent::ClaudeCode, None), "claude");
        assert!(is_sandbox_name(&named("/src/Lockbox")));
        assert!(!is_sandbox_name("Claude"));
        assert!(!is_sandbox_name("1claude"));
        assert!(!is_sandbox_name("claude-"));
        assert!(!is_sandbox_name(""));
    }

    #[test]
    fn opening_runs_the_agent_in_its_folder_or_a_shell_in_a_sandbox_lockbox_didnt_make() {
        let sandbox = Sandbox {
            name: "claude-patchbay".to_string(),
            phase: Phase::Ready,
            created: String::new(),
            agent: Some("claude-code".to_string()),
            folder: None,
            workdir: Some("/sandbox/Sam's App".to_string()),
        };
        let openshell = Path::new("/opt/homebrew/bin/openshell");
        insta::assert_snapshot!(open_script(openshell, &sandbox));
        let theirs = Sandbox {
            agent: None,
            ..sandbox
        };
        assert!(open_script(openshell, &theirs).ends_with("sandbox connect 'claude-patchbay'\n"));
    }

    #[test]
    fn an_agent_is_found_by_its_label() {
        assert_eq!(Agent::from_id("claude-code"), Some(Agent::ClaudeCode));
        assert_eq!(Agent::from_id("codex"), None);
    }
}
