//! The agents Playpen runs in a sandbox, and making a sandbox for one: its image, the policy that lets it reach its
//! model and nothing else, and the folder it works on.

use std::path::{Path, PathBuf};

use crate::chatgpt::Account;
use crate::docker::{Docker, DockerError};
use crate::kits::{Variable, VariableValue};
use crate::new_service::find_op;
use crate::openshell::{
    AGENT_LABEL, FOLDER_ANNOTATION, OpenShell, OpenShellError, Sandbox, Service, WORKDIR_ANNOTATION,
};
use crate::secrets::Secret;
use crate::session::What;

/// The Claude Code release the image is built with. Raising it builds a new image the next time one is needed.
const CLAUDE_CODE_VERSION: &str = "2.1.329";

/// The Codex release its image is built with.
const CODEX_VERSION: &str = "0.171.0";

/// Codex's real program, which npm installs for the sandbox's architecture: the Mac's, since Docker on a Mac runs
/// Linux for the Mac's own chip.
const CODEX_BINARY: &str = if cfg!(target_arch = "aarch64") {
    "/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-arm64/vendor/aarch64-unknown-linux-musl/bin/codex"
} else {
    "/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex"
};

/// Where a sandbox's own files are, and where the folders are copied to.
const SANDBOX_HOME: &str = "/sandbox";

/// An agent Playpen knows how to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Agent {
    ClaudeCode,
    Codex,
}

impl Agent {
    pub const ALL: [Agent; 2] = [Agent::ClaudeCode, Agent::Codex];

    /// Its name in a sandbox's label.
    pub fn id(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude-code",
            Agent::Codex => "codex",
        }
    }

    pub fn from_id(id: &str) -> Option<Agent> {
        Agent::ALL.into_iter().find(|agent| agent.id() == id)
    }

    pub fn name(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "Claude Code",
            Agent::Codex => "Codex",
        }
    }

    /// The image it runs in, tagged with the agent's version.
    pub fn image(self) -> String {
        match self {
            Agent::ClaudeCode => format!("playpen-claude-code:{CLAUDE_CODE_VERSION}"),
            Agent::Codex => format!("playpen-codex:{CODEX_VERSION}"),
        }
    }

    /// The version of the agent its image is built with.
    pub fn version(self) -> &'static str {
        match self {
            Agent::ClaudeCode => CLAUDE_CODE_VERSION,
            Agent::Codex => CODEX_VERSION,
        }
    }

    /// Builds its image, or builds it again over the one there, as making a sandbox does when there's none.
    pub fn build_image(self, docker: &Docker) -> Result<(), DockerError> {
        docker.build(&self.image(), self.dockerfile(), &self.build_args())
    }

    fn dockerfile(self) -> &'static str {
        match self {
            Agent::ClaudeCode => include_str!("claude-code.Dockerfile"),
            Agent::Codex => include_str!("codex.Dockerfile"),
        }
    }

    fn build_args(self) -> Vec<(&'static str, &'static str)> {
        match self {
            Agent::ClaudeCode => vec![("CLAUDE_CODE_VERSION", CLAUDE_CODE_VERSION)],
            Agent::Codex => vec![("CODEX_VERSION", CODEX_VERSION)],
        }
    }

    /// The command that starts it.
    pub fn command(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude",
            Agent::Codex => "codex",
        }
    }

    /// The command that carries on its last conversation in the folder.
    fn continue_command(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude --continue",
            Agent::Codex => "codex resume --last",
        }
    }

    /// The program that reaches the model, by its real path in the image. OpenShell matches a rule's binaries by
    /// the path a connection really came from, so `/usr/local/bin/claude`, a link to this, matches nothing.
    fn binary(self) -> &'static str {
        match self {
            Agent::ClaudeCode => {
                "/usr/local/lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe"
            }
            Agent::Codex => CODEX_BINARY,
        }
    }

    /// The types of provider that hold a key it can use: a subscription's token, or an API key.
    pub fn provider_kinds(self) -> &'static [&'static str] {
        match self {
            Agent::ClaudeCode => &[
                "claude-code-oauth",
                "claude-code",
                "anthropic",
                "playpen-anthropic",
            ],
            Agent::Codex => &["playpen-chatgpt", "playpen-openai"],
        }
    }

    /// The hosts it needs: its model's API, and, for Claude Code, the service it checks for when it starts
    /// interactively, which it won't start without.
    fn hosts(self) -> &'static [&'static str] {
        match self {
            Agent::ClaudeCode => &["api.anthropic.com", "platform.claude.com"],
            Agent::Codex => &["chatgpt.com", "api.openai.com"],
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
            Agent::Codex => &[],
        }
    }

    /// The sandbox's policy: the hosts it needs, from its own binary, and nothing else. A provider adds its own
    /// rules, but one written for another image names another path, so the agent's are written here too.
    pub fn policy(self) -> String {
        let mut policy = format!(
            "# Written by Playpen. Everything this doesn't allow is denied.\n\
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
    /// The ChatGPT account, when the provider is a ChatGPT subscription.
    pub account: Option<Account>,
    /// Services to attach besides the model's key, each with its hosts opened.
    pub services: Vec<Service>,
    /// Variables to set in it: values, and secrets fetched from 1Password or the Keychain as it's made.
    pub variables: Vec<Variable>,
    /// Folders on the Mac to copy in, for it to work on: none, one, or several, such as the repositories of one
    /// piece of work.
    pub folders: Vec<PathBuf>,
}

/// How far making a sandbox has got, for showing while it waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Building the agent's image, which happens once for each version.
    Building,
    Creating,
    /// Copying the folders in.
    Copying,
    /// Writing the agent's settings in the sandbox.
    Preparing,
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
        "“{0}” can't name a sandbox: use up to 19 lowercase letters, digits and dashes, starting with a letter."
    )]
    Name(String),
    #[error(
        "Two of the folders are called “{0}”, and each is copied in under its own name. Leave one out, or rename it."
    )]
    SameFolderName(String),
}

impl Plan {
    /// The sandbox's policy: its agent reaching its model, and each service's hosts.
    pub fn policy(&self) -> String {
        let mut policy = self.agent.policy();
        for service in &self.services {
            policy.push_str(&service.policy());
        }
        policy
    }

    /// Where the agent works in the sandbox: always its home, where the folders are side by side, so one added later
    /// is beside the rest rather than outside where the agent started.
    pub fn workdir(&self) -> String {
        SANDBOX_HOME.to_string()
    }

    /// A name two of the folders share, which would put one on top of the other in the sandbox.
    fn shared_folder_name(&self) -> Option<String> {
        let mut seen = std::collections::BTreeSet::new();
        self.folders
            .iter()
            .filter_map(|folder| folder.file_name())
            .find(|name| !seen.insert(*name))
            .map(|name| name.to_string_lossy().into_owned())
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
        // One a line, which OpenShell keeps as it's given.
        if !self.folders.is_empty() {
            let folders: Vec<String> = self
                .folders
                .iter()
                .map(|folder| folder.display().to_string())
                .collect();
            args.extend([
                "--annotation".to_string(),
                format!("{FOLDER_ANNOTATION}={}", folders.join("\n")),
            ]);
        }
        if let Some(provider) = &self.provider {
            args.extend(["--provider".to_string(), provider.clone()]);
        }
        for service in &self.services {
            args.extend(["--provider".to_string(), service.provider.clone()]);
        }
        // A secret is taken from `openshell`'s own environment, where it's fetched to, so it's never an argument.
        for variable in &self.variables {
            match &variable.value {
                VariableValue::Value(value) => {
                    args.extend(["--env".to_string(), format!("{}={value}", variable.name)]);
                }
                VariableValue::Reference(_) | VariableValue::Keychain(_) => {
                    args.extend(["--env-from".to_string(), variable.name.clone()]);
                }
            }
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
        if let Some(name) = self.shared_folder_name() {
            return Err(MakeError::SameFolderName(name));
        }
        let image = self.agent.image();
        if !docker.has_image(&image)? {
            step(Step::Building);
            self.agent.build_image(docker)?;
        }
        step(Step::Creating);
        let policy = std::env::temp_dir().join(format!(
            "playpen-policy-{}-{}.yaml",
            std::process::id(),
            self.name
        ));
        std::fs::write(&policy, self.policy())
            .map_err(|error| MakeError::Policy(error.to_string()))?;
        let secrets: Vec<(String, Secret)> = self
            .variables
            .iter()
            .filter_map(|variable| Some((variable.name.clone(), variable.secret()?)))
            .collect();
        let created = openshell.create_with_secrets(
            &self.name,
            &self.create_args(&policy),
            &secrets,
            find_op().as_deref(),
        );
        let _ = std::fs::remove_file(&policy);
        created?;
        // A sandbox without its folders or its settings isn't what was asked for, and would keep the name from being
        // tried again.
        if !self.folders.is_empty() {
            step(Step::Copying);
        }
        for folder in &self.folders {
            if let Err(error) = openshell.upload(&self.name, folder, SANDBOX_HOME) {
                let _ = openshell.delete(&self.name);
                return Err(error.into());
            }
        }
        if let Some(setup) = self.setup() {
            step(Step::Preparing);
            let input = setup.to_string();
            if let Err(error) = openshell.exec_with_input(
                &self.name,
                &format!("set up {} in “{}”", self.agent.name(), self.name),
                &["node", "-e", SETUP_SCRIPT],
                input.as_bytes(),
            ) {
                let _ = openshell.delete(&self.name);
                return Err(error.into());
            }
        }
        Ok(())
    }

    /// What the agent needs written in the sandbox before it starts, given to [`SETUP_SCRIPT`]; none for Claude Code.
    ///
    /// Codex runs with its own sandbox off, since OpenShell's is the one that counts and Codex's needs kernel features
    /// a sandbox doesn't have. On a ChatGPT subscription it gets Codex's own `auth.json`, holding OpenShell's
    /// stand-in for the access token, which OpenShell swaps for the real one on Codex's requests and renews, and a
    /// refresh token that isn't one, since OpenShell holds that. Its last refresh is put far ahead so Codex never
    /// tries to renew the login itself, which it couldn't: the sandbox can't reach OpenAI's sign-in.
    fn setup(&self) -> Option<serde_json::Value> {
        match self.agent {
            Agent::ClaudeCode => None,
            Agent::Codex => Some(serde_json::json!({
                "config": "# Written by Playpen.\nsandbox_mode = \"danger-full-access\"\ncheck_for_update_on_startup = false\n",
                "account": self.account,
                "sign_in": SIGN_IN_SCRIPT,
            })),
        }
    }
}

/// Writes an agent's settings in its sandbox, from what [`Plan::setup`] gives on its standard input. It runs in the
/// sandbox, where the stand-in for a provider's key is in the environment.
const SETUP_SCRIPT: &str = r#"
const fs = require("fs");
const setup = JSON.parse(fs.readFileSync(0, "utf8"));
const home = "/sandbox/.codex";
fs.mkdirSync(home, { recursive: true, mode: 0o700 });
fs.writeFileSync(home + "/config.toml", setup.config);
fs.writeFileSync(home + "/playpen-sign-in.js", setup.sign_in);
if (setup.account) {
  if (!process.env.CHATGPT_ACCESS_TOKEN) throw new Error("the ChatGPT provider's token isn't in the sandbox");
  fs.writeFileSync(home + "/auth.json", JSON.stringify({
    auth_mode: "chatgpt",
    OPENAI_API_KEY: null,
    tokens: {
      id_token: setup.account.id_token,
      access_token: "",
      refresh_token: "held-by-openshell",
      account_id: setup.account.account_id,
    },
    last_refresh: "2099-01-01T00:00:00Z",
  }), { mode: 0o600 });
  require(home + "/playpen-sign-in.js");
}
"#;

/// Puts the sandbox's current stand-in for the ChatGPT access token in Codex's `auth.json`, run before Codex each
/// time it's opened. OpenShell gives each new process the current stand-in, and the one in the file would go stale
/// when the person signs in again; it can't be kept stable, since OpenShell renews the token.
const SIGN_IN_SCRIPT: &str = r#"
const fs = require("fs");
const file = "/sandbox/.codex/auth.json";
const access = process.env.CHATGPT_ACCESS_TOKEN;
if (access && fs.existsSync(file)) {
  const auth = JSON.parse(fs.readFileSync(file, "utf8"));
  if (auth.tokens && auth.tokens.access_token !== access) {
    auth.tokens.access_token = access;
    fs.writeFileSync(file, JSON.stringify(auth), { mode: 0o600 });
  }
}
"#;

/// The longest name OpenShell 0.1.8 takes for a sandbox, as it says when given a longer one.
pub const SANDBOX_NAME_MAX: usize = 19;

/// Whether OpenShell takes `name` for a sandbox: a DNS label, as a container's name is, of no more than
/// [`SANDBOX_NAME_MAX`] characters.
pub fn is_sandbox_name(name: &str) -> bool {
    (1..=SANDBOX_NAME_MAX).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A sandbox name made from the folders' names, for `agent`: `claude-patchbay` for a folder `Patchbay`, and
/// `claude-app-web` for `app` and `web`. Since OpenShell's names are short, it takes as many whole folder names as
/// fit, cutting the first short only when even that doesn't.
pub fn suggested_name(agent: Agent, folders: &[PathBuf]) -> String {
    let mut name = agent.command().to_string();
    for folder in folders.iter().filter_map(|folder| folder.file_name()) {
        let mut part = String::new();
        for c in folder.to_string_lossy().chars() {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() {
                part.push(c);
            } else if !part.is_empty() && !part.ends_with('-') {
                part.push('-');
            }
        }
        let part = part.trim_end_matches('-');
        if part.is_empty() {
            continue;
        }
        let longer = format!("{name}-{part}");
        if longer.len() <= SANDBOX_NAME_MAX {
            name = longer;
        } else {
            if name == agent.command() {
                name = longer;
                name.truncate(SANDBOX_NAME_MAX);
            }
            break;
        }
    }
    name.trim_end_matches('-').to_string()
}

/// The shell line that runs `what` in the sandbox, in its agent's folder. A sandbox with no agent Playpen knows only
/// gets a shell, through `sandbox connect`, since its image may have no `bash`.
pub fn open_command(openshell: &Path, sandbox: &Sandbox, what: What) -> String {
    let openshell = quote(&openshell.to_string_lossy());
    let name = quote(&sandbox.name);
    let Some(agent) = sandbox.agent.as_deref().and_then(Agent::from_id) else {
        return format!("{openshell} sandbox connect {name}");
    };
    let run = match what {
        What::Agent => agent.command().to_string(),
        What::Continue => agent.continue_command().to_string(),
        What::Shell => "bash -l".to_string(),
    };
    let run = match (agent, what) {
        // Codex's login is brought up to date first, as `SIGN_IN_SCRIPT` says.
        (Agent::Codex, What::Agent | What::Continue) => format!(
            "sh -c {}",
            quote(&format!(
                "node /sandbox/.codex/playpen-sign-in.js 2>/dev/null; exec {run}"
            ))
        ),
        _ => run,
    };
    format!(
        "{openshell} sandbox exec --name {name} --tty --workdir {workdir} -- {run}",
        workdir = quote(sandbox.workdir.as_deref().unwrap_or(SANDBOX_HOME)),
    )
}

/// `text` as one word to a POSIX shell.
pub(crate) fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Agent, Plan, is_sandbox_name, open_command, suggested_name};
    use crate::openshell::{Phase, Sandbox};
    use crate::session::What;

    fn plan() -> Plan {
        Plan {
            name: "claude-patchbay".to_string(),
            agent: Agent::ClaudeCode,
            provider: Some("claude-sub".to_string()),
            account: None,
            services: Vec::new(),
            variables: Vec::new(),
            folders: vec![PathBuf::from("/Users/sam/src/Patchbay")],
        }
    }

    #[test]
    fn claude_codes_policy_allows_its_model_from_its_real_binary_and_nothing_else() {
        insta::assert_snapshot!(Agent::ClaudeCode.policy());
    }

    #[test]
    fn codexs_policy_allows_chatgpt_and_openai_from_its_real_binary() {
        let policy = Agent::Codex.policy();
        assert!(policy.contains("host: chatgpt.com") && policy.contains("host: api.openai.com"));
        assert!(
            policy.contains("/bin/codex\n"),
            "the binary, not codex.js: {policy}"
        );
    }

    #[test]
    fn codex_on_a_chatgpt_subscription_is_set_up_with_the_account_and_its_sandbox_off() {
        let codex = Plan {
            agent: Agent::Codex,
            account: Some(crate::chatgpt::Account {
                account_id: "acct-1".to_string(),
                id_token: "x.y.".to_string(),
            }),
            ..plan()
        };
        let setup = codex.setup().unwrap();
        assert_eq!(setup["account"]["account_id"], "acct-1");
        assert!(
            setup["config"]
                .as_str()
                .unwrap()
                .contains("danger-full-access")
        );
        assert!(plan().setup().is_none(), "Claude Code needs none");
    }

    #[test]
    fn a_sandbox_is_made_detached_labelled_and_waiting_for_approval() {
        let args = plan().create_args(Path::new("/tmp/policy.yaml"));
        insta::assert_snapshot!(args.join("\n"));
    }

    #[test]
    fn the_agent_works_above_the_folders_however_many_there_are() {
        assert_eq!(plan().workdir(), "/sandbox");
        let none = Plan {
            folders: Vec::new(),
            ..plan()
        };
        assert_eq!(none.workdir(), "/sandbox");
    }

    #[test]
    fn several_folders_are_copied_in_side_by_side_and_the_agent_works_above_them() {
        let several = Plan {
            folders: vec![
                PathBuf::from("/Users/sam/src/Patchbay"),
                PathBuf::from("/Users/sam/src/Typecase"),
            ],
            ..plan()
        };
        assert_eq!(several.workdir(), "/sandbox");
        assert!(several.create_args(Path::new("/tmp/policy.yaml")).contains(
            &"playpen.folder=/Users/sam/src/Patchbay\n/Users/sam/src/Typecase".to_string()
        ));
        assert_eq!(several.shared_folder_name(), None);
    }

    #[test]
    fn two_folders_of_one_name_are_refused_before_anything_is_made() {
        let clashing = Plan {
            folders: vec![
                PathBuf::from("/Users/sam/src/app"),
                PathBuf::from("/Users/sam/work/app"),
            ],
            ..plan()
        };
        assert_eq!(clashing.shared_folder_name().as_deref(), Some("app"));
    }

    #[test]
    fn a_folders_name_becomes_a_sandboxs() {
        let named = |folder: &str| suggested_name(Agent::ClaudeCode, &[PathBuf::from(folder)]);
        assert_eq!(named("/src/Patchbay"), "claude-patchbay");
        assert_eq!(named("/src/my app (2)"), "claude-my-app-2");
        assert_eq!(suggested_name(Agent::ClaudeCode, &[]), "claude");
        let several = |folders: &[&str]| {
            let folders: Vec<PathBuf> = folders.iter().map(PathBuf::from).collect();
            suggested_name(Agent::ClaudeCode, &folders)
        };
        assert_eq!(several(&["/src/app", "/src/web"]), "claude-app-web");
        assert_eq!(
            several(&["/src/Patchbay", "/src/Typecase"]),
            "claude-patchbay",
            "as many whole names as fit"
        );
        assert_eq!(
            several(&["/src/A Very Long Project Name"]),
            "claude-a-very-long"
        );
        assert!(!is_sandbox_name("claude-patchbay-type"), "20 characters");
        assert!(is_sandbox_name(&named("/src/Playpen")));
        assert!(!is_sandbox_name("Claude"));
        assert!(!is_sandbox_name("1claude"));
        assert!(!is_sandbox_name("claude-"));
        assert!(!is_sandbox_name(""));
    }

    #[test]
    fn opening_runs_the_agent_in_its_folder_or_a_shell_in_a_sandbox_playpen_didnt_make() {
        let sandbox = Sandbox {
            name: "claude-patchbay".to_string(),
            phase: Phase::Ready,
            created: String::new(),
            agent: Some("claude-code".to_string()),
            folders: Vec::new(),
            workdir: Some("/sandbox/Sam's App".to_string()),
        };
        let openshell = Path::new("/opt/homebrew/bin/openshell");
        insta::assert_snapshot!(open_command(openshell, &sandbox, What::Agent));
        assert!(
            open_command(openshell, &sandbox, What::Continue).ends_with("-- claude --continue")
        );
        assert!(open_command(openshell, &sandbox, What::Shell).ends_with("-- bash -l"));
        let theirs = Sandbox {
            agent: None,
            ..sandbox
        };
        assert!(
            open_command(openshell, &theirs, What::Continue)
                .ends_with("sandbox connect 'claude-patchbay'")
        );
    }

    #[test]
    fn codex_is_opened_after_its_login_is_brought_up_to_date() {
        let sandbox = Sandbox {
            name: "codex-patchbay".to_string(),
            phase: Phase::Ready,
            created: String::new(),
            agent: Some("codex".to_string()),
            folders: Vec::new(),
            workdir: Some("/sandbox/Patchbay".to_string()),
        };
        let openshell = Path::new("/opt/homebrew/bin/openshell");
        insta::assert_snapshot!(open_command(openshell, &sandbox, What::Continue));
        assert!(open_command(openshell, &sandbox, What::Shell).ends_with("-- bash -l"));
    }

    #[test]
    fn an_agent_is_found_by_its_label() {
        assert_eq!(Agent::from_id("claude-code"), Some(Agent::ClaudeCode));
        assert_eq!(Agent::from_id("codex"), Some(Agent::Codex));
        assert_eq!(Agent::from_id("cursor"), None);
    }
}
