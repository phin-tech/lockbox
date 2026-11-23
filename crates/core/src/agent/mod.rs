//! The agents Playpen runs in a sandbox, and making a sandbox for one: its image, the policy that lets it reach its
//! model and nothing else, and the folder it works on.

mod custom;

use std::path::{Path, PathBuf};

pub use custom::{CustomAgent, CustomAgentError, CustomAgents};

use crate::bring_back::Repo;
use crate::chatgpt::Account;
use crate::docker::{Docker, DockerError};
use crate::image::{Image, ImageSource, Platform};
use crate::kit_setup::{Base, SetupFolder, image_name, link_dotfiles};
use crate::kits::{Variable, VariableValue};
use crate::new_service::find_op;
use crate::openshell::{
    AGENT_LABEL, COMMAND_ANNOTATION, CONTINUE_ANNOTATION, CUSTOM_NAME_ANNOTATION,
    FOLDER_ANNOTATION, GATEWAY_VARIABLE, Gateway, OpenShell, OpenShellError, Profile, Provider,
    Sandbox, Service, WORKDIR_ANNOTATION,
};
use crate::provider::{Endpoint, Kind};
use crate::secrets::Secret;
use crate::session::What;

/// The Claude Code release the image is built with. Raising it builds a new image the next time one is needed.
const CLAUDE_CODE_VERSION: &str = "2.1.329";

/// The Codex release its image is built with.
const CODEX_VERSION: &str = "0.171.0";

/// The Pi release its image is built with, from npm.
const PI_VERSION: &str = "1.5.0";

/// The oh-my-pi release its image is built with, and the digests its release published for each platform's program.
const OMP_VERSION: &str = "18.22.2";
const OMP_SHA256_X64: &str = "80a692c5adb9c71124bb11e77bd7edb3cf8fbbab6980db3206dcb6a4cb2905b5";
const OMP_SHA256_ARM64: &str = "82a880ba04d73f1c81a12931447f9f9ed8945581998564eae900c1238f4502e2";

/// The OpenCode release its image is built with, and the digests its release published for each platform's archive.
const OPENCODE_VERSION: &str = "1.19.6";
const OPENCODE_SHA256_X64: &str =
    "47734e08968b30cc22a0bffb6d9feeed76ad4aa743213a12122040392d09a057";
const OPENCODE_SHA256_ARM64: &str =
    "5b68c1f4c5e75b4134e664cc8f36d634d64208d4fcfe5bcf3b48bab699c2dbac";

/// The Cursor agent release its image is built with, and the digests Cursor published for each platform's archive.
const CURSOR_VERSION: &str = "2026.11.22-e9042b2";
const CURSOR_SHA256_X64: &str = "7663b1c5d1b56e095ef8e23bef38d66df6cfd665c4873470214e0e411f0f97b3";
const CURSOR_SHA256_ARM64: &str =
    "7b7dd0020fb9a2a9e1e7e3f39e2f9f512d1dd9577174ecda8aa82ac31dbc9565";

/// The PiG release its image is built with, and the digests its release published for each platform's archive.
const PIG_VERSION: &str = "0.7.0";
const PIG_SHA256_AMD64: &str = "0b5ab7cb5f680e7d3353bdaa3a1018863c3c3552f88bec730370668e5819995a";
const PIG_SHA256_ARM64: &str = "88d2dcb4a4852a980dc1ca9202eed9455ff8fb011553c8db62c1b85c676a366b";

/// Codex's real program, which npm installs for the image's platform, so the path depends on it.
fn codex_binary(platform: Platform) -> &'static str {
    match platform {
        Platform::Arm64 => {
            "/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-arm64/vendor/aarch64-unknown-linux-musl/bin/codex"
        }
        Platform::Amd64 => {
            "/usr/local/lib/node_modules/@openai/codex/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex"
        }
    }
}

/// Where a sandbox's own files are, and where the folders are copied to.
const SANDBOX_HOME: &str = "/sandbox";

/// An agent Playpen knows how to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Agent {
    ClaudeCode,
    Codex,
    /// Pi, which reaches whichever model service its key is for.
    Pi,
    /// oh-my-pi, a fork of Pi.
    Omp,
    /// PiG, Pi written again in Go.
    Pig,
    /// OpenCode, which reaches whichever model service its key is for.
    OpenCode,
    /// Cursor's agent, which reaches only Cursor's service, on a Cursor key.
    Cursor,
}

impl Agent {
    pub const ALL: [Agent; 7] = [
        Agent::ClaudeCode,
        Agent::Codex,
        Agent::OpenCode,
        Agent::Cursor,
        Agent::Pi,
        Agent::Omp,
        Agent::Pig,
    ];

    /// Its name in a sandbox's label.
    pub fn id(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude-code",
            Agent::Codex => "codex",
            Agent::Pi => "pi",
            Agent::Omp => "omp",
            Agent::Pig => "pig",
            Agent::OpenCode => "opencode",
            Agent::Cursor => "cursor",
        }
    }

    pub fn from_id(id: &str) -> Option<Agent> {
        Agent::ALL.into_iter().find(|agent| agent.id() == id)
    }

    pub fn name(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "Claude Code",
            Agent::Codex => "Codex",
            Agent::Pi => "Pi",
            Agent::Omp => "oh-my-pi",
            Agent::Pig => "PiG",
            Agent::OpenCode => "OpenCode",
            Agent::Cursor => "Cursor",
        }
    }

    /// The image it runs in, tagged with the agent's version and what its Dockerfile says, so a changed Dockerfile
    /// makes a new image rather than using the one built before.
    pub fn image(self) -> String {
        let name = match self {
            Agent::ClaudeCode => "playpen-claude-code",
            Agent::Codex => "playpen-codex",
            Agent::Pi => "playpen-pi",
            Agent::Omp => "playpen-omp",
            Agent::Pig => "playpen-pig",
            Agent::OpenCode => "playpen-opencode",
            Agent::Cursor => "playpen-cursor",
        };
        format!(
            "{name}:{}-{:08x}",
            self.version(),
            fnv1a(self.dockerfile().as_bytes())
        )
    }

    /// The version of the agent its image is built with.
    pub fn version(self) -> &'static str {
        match self {
            Agent::ClaudeCode => CLAUDE_CODE_VERSION,
            Agent::Codex => CODEX_VERSION,
            Agent::Pi => PI_VERSION,
            Agent::Omp => OMP_VERSION,
            Agent::Pig => PIG_VERSION,
            Agent::OpenCode => OPENCODE_VERSION,
            Agent::Cursor => CURSOR_VERSION,
        }
    }

    /// Builds its image, or builds it again over the one there, as making a sandbox does when there's none.
    pub fn build_image(self, docker: &Docker) -> Result<(), DockerError> {
        docker.build(&self.image(), self.dockerfile(), &self.build_args(), None)
    }

    /// What an image source needs to have its image ready.
    pub(crate) fn build(self) -> Image {
        Image {
            tag: self.image(),
            dockerfile: self.dockerfile(),
            args: self.build_args(),
        }
    }

    fn dockerfile(self) -> &'static str {
        match self {
            Agent::ClaudeCode => include_str!("claude-code.Dockerfile"),
            Agent::Codex => include_str!("codex.Dockerfile"),
            Agent::Pi => include_str!("pi.Dockerfile"),
            Agent::Omp => include_str!("omp.Dockerfile"),
            Agent::Pig => include_str!("pig.Dockerfile"),
            Agent::OpenCode => include_str!("opencode.Dockerfile"),
            Agent::Cursor => include_str!("cursor.Dockerfile"),
        }
    }

    fn build_args(self) -> Vec<(&'static str, &'static str)> {
        match self {
            Agent::ClaudeCode => vec![("CLAUDE_CODE_VERSION", CLAUDE_CODE_VERSION)],
            Agent::Codex => vec![("CODEX_VERSION", CODEX_VERSION)],
            Agent::Pi => vec![("PI_VERSION", PI_VERSION)],
            Agent::Omp => vec![
                ("OMP_VERSION", OMP_VERSION),
                ("OMP_SHA256_X64", OMP_SHA256_X64),
                ("OMP_SHA256_ARM64", OMP_SHA256_ARM64),
            ],
            Agent::Pig => vec![
                ("PIG_VERSION", PIG_VERSION),
                ("PIG_SHA256_AMD64", PIG_SHA256_AMD64),
                ("PIG_SHA256_ARM64", PIG_SHA256_ARM64),
            ],
            Agent::OpenCode => vec![
                ("OPENCODE_VERSION", OPENCODE_VERSION),
                ("OPENCODE_SHA256_X64", OPENCODE_SHA256_X64),
                ("OPENCODE_SHA256_ARM64", OPENCODE_SHA256_ARM64),
            ],
            Agent::Cursor => vec![
                ("CURSOR_VERSION", CURSOR_VERSION),
                ("CURSOR_SHA256_X64", CURSOR_SHA256_X64),
                ("CURSOR_SHA256_ARM64", CURSOR_SHA256_ARM64),
            ],
        }
    }

    /// What a sandbox for it is named after.
    fn name_stem(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude",
            Agent::Codex => "codex",
            Agent::Pi => "pi",
            Agent::Omp => "omp",
            Agent::Pig => "pig",
            Agent::OpenCode => "opencode",
            Agent::Cursor => "cursor",
        }
    }

    /// The command that starts it. Codex runs without the background server it would keep for its sessions, which
    /// needs more of the processes around it than a sandbox shows, and which a sandbox's session has no use for; and
    /// with SQLite, which it keeps its state in, putting its temporary files in /tmp rather than /var/tmp, which a
    /// sandbox may only read. It's said on the command, since `sandbox exec` doesn't pass on the image's environment.
    pub fn command(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude",
            Agent::Codex => "env SQLITE_TMPDIR=/tmp TMPDIR=/tmp codex --no-daemon",
            Agent::Pi => "pi",
            // The sandbox's home is where its folders are, and oh-my-pi would otherwise start in a folder of its own
            // rather than in a home.
            Agent::Omp => "omp --allow-home",
            Agent::Pig => "pig",
            Agent::OpenCode => "opencode",
            Agent::Cursor => "cursor-agent",
        }
    }

    /// The command that carries on its last conversation in the folder.
    fn continue_command(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude --continue",
            Agent::Codex => "env SQLITE_TMPDIR=/tmp TMPDIR=/tmp codex resume --last --no-daemon",
            Agent::Pi => "pi --continue",
            Agent::Omp => "omp --allow-home --continue",
            Agent::Pig => "pig --continue",
            Agent::OpenCode => "opencode --continue",
            Agent::Cursor => "cursor-agent --continue",
        }
    }

    /// The program that reaches the model, by its real path in the image. OpenShell matches a rule's binaries by
    /// the path a connection really came from, so `/usr/local/bin/claude`, a link to this, matches nothing. Pi is a
    /// Node script, so its program is node itself, and any other Node script in the sandbox may reach its model's
    /// host too; OpenShell still adds the key only on the hosts the key is for.
    fn binary(self, platform: Platform) -> &'static str {
        match self {
            Agent::ClaudeCode => {
                "/usr/local/lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe"
            }
            Agent::Codex => codex_binary(platform),
            Agent::Pi => "/usr/local/bin/node",
            Agent::Omp => "/usr/local/bin/omp",
            Agent::Pig => "/usr/local/bin/pig",
            Agent::OpenCode => "/usr/local/bin/opencode",
            // Cursor's own Node, which its launcher runs.
            Agent::Cursor => "/opt/cursor-agent/node",
        }
    }

    /// The program the policy lets reach its model, in an image for this Mac's platform, which an image of the
    /// person's own has to have.
    pub fn program(self) -> &'static str {
        self.binary(Platform::host())
    }

    /// The kinds of provider Playpen adds that hold a key it can use, the one to suggest first. Pi and its kin take
    /// API keys only: OpenShell gives a sandbox a stand-in for a key, and they tell a subscription's token by what's in
    /// it, which a stand-in doesn't show.
    pub fn model_kinds(self) -> &'static [Kind] {
        match self {
            Agent::ClaudeCode => &[Kind::ClaudeSubscription, Kind::Anthropic],
            Agent::Codex => &[Kind::ChatGptSubscription, Kind::OpenAi],
            Agent::Pi | Agent::Omp | Agent::Pig | Agent::OpenCode => &[
                Kind::OpenRouter,
                Kind::Anthropic,
                Kind::OpenAi,
                Kind::LiteLlm,
            ],
            Agent::Cursor => &[Kind::Cursor],
        }
    }

    /// Whether a provider OpenShell lists with the type `kind` holds a key it can use. Claude Code also takes the
    /// providers OpenShell makes itself, which aren't Playpen's kinds.
    pub fn takes(self, kind: &str) -> bool {
        Kind::of(kind).is_some_and(|known| self.model_kinds().contains(&known))
            || (self == Agent::ClaudeCode && matches!(kind, "claude-code" | "anthropic"))
    }

    /// Whether the person says which model it starts with, since it reaches several services. It's needed for a
    /// LiteLLM proxy, whose models only the proxy knows.
    pub fn chooses_a_model(self) -> bool {
        matches!(self, Agent::Pi | Agent::Omp | Agent::Pig | Agent::OpenCode)
    }

    /// The hosts it needs whatever its key: its model's API, and, for Claude Code, the service it checks for when it
    /// starts interactively, which it won't start without. Pi and its kin reach only the host their key is for.
    fn hosts(self) -> &'static [&'static str] {
        match self {
            Agent::ClaudeCode => &["api.anthropic.com", "platform.claude.com"],
            Agent::Codex => &["chatgpt.com", "api.openai.com"],
            Agent::Pi | Agent::Omp | Agent::Pig | Agent::OpenCode => &[],
            // Only Cursor's API, which every request goes to. Its metrics, error reports and updates are denied.
            Agent::Cursor => &["api2.cursor.sh"],
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
            // Nothing at startup: no version check, model catalogue or count of installs, which the policy would only
            // deny.
            Agent::Pi => &[
                ("PI_OFFLINE", "1"),
                ("PI_SKIP_VERSION_CHECK", "1"),
                ("PI_TELEMETRY", "0"),
            ],
            Agent::Omp => &[("PI_AUTO_QA", "0"), ("PI_TELEMETRY", "0")],
            Agent::Pig => &[
                ("PIG_OFFLINE", "1"),
                ("PI_SKIP_VERSION_CHECK", "1"),
                ("PI_TELEMETRY", "0"),
            ],
            Agent::OpenCode => &[
                ("OPENCODE_DISABLE_MODELS_FETCH", "1"),
                ("OPENCODE_DISABLE_AUTOUPDATE", "1"),
                ("OPENCODE_DISABLE_LSP_DOWNLOAD", "1"),
            ],
            Agent::Cursor => &[],
        }
    }

    /// The sandbox's policy: the hosts it needs, from its own binary in an image for `platform`, and nothing else. A
    /// provider adds its own rules, but one written for another image names another path, so the agent's are written
    /// here too.
    pub fn policy(self, platform: Platform) -> String {
        self.policy_reaching(platform, &[])
    }

    /// Its policy, reaching `model` too: the hosts its key is for, when it reaches whichever service that is.
    fn policy_reaching(self, platform: Platform, model: &[Endpoint]) -> String {
        let fixed = self.hosts().iter().map(|host| ((*host).to_string(), 443));
        let keyed = model
            .iter()
            .map(|endpoint| (endpoint.host.clone(), endpoint.port));
        policy_text(self.id(), fixed.chain(keyed), self.binary(platform))
    }
}

/// A policy letting `binary` reach each host and port under the rule `rule`, and nothing else.
fn policy_text(rule: &str, hosts: impl Iterator<Item = (String, u16)>, binary: &str) -> String {
    let mut policy = format!(
        "# Written by Playpen. Everything this doesn't allow is denied.\n\
         version: 1\n\
         network_policies:\n\
         \x20 {rule}:\n\
         \x20   endpoints:\n",
    );
    for (host, port) in hosts {
        policy.push_str(&format!(
            "      - host: {host}\n        port: {port}\n        protocol: rest\n        access: read-write\n        enforcement: enforce\n"
        ));
    }
    policy.push_str(&format!("    binaries:\n      - path: {binary}\n"));
    policy
}

/// The label a sandbox for a custom agent has, whose own name and commands are in its annotations.
pub const CUSTOM_LABEL: &str = "custom";

/// What a sandbox runs: one of Playpen's agents, or one of the person's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Harness {
    Agent(Agent),
    Custom(CustomAgent),
}

impl From<Agent> for Harness {
    fn from(agent: Agent) -> Harness {
        Harness::Agent(agent)
    }
}

impl Harness {
    /// Its sandboxes' label.
    pub fn label(&self) -> &str {
        match self {
            Harness::Agent(agent) => agent.id(),
            Harness::Custom(_) => CUSTOM_LABEL,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Harness::Agent(agent) => agent.name(),
            Harness::Custom(custom) => &custom.name,
        }
    }

    /// Whether a provider OpenShell lists with the type `kind` holds a key it can use.
    pub fn takes(&self, kind: &str) -> bool {
        match self {
            Harness::Agent(agent) => agent.takes(kind),
            Harness::Custom(custom) => custom.takes(kind),
        }
    }

    /// Whether it reaches whichever service its key is for, so the key's hosts are opened for it.
    pub fn chooses_a_model(&self) -> bool {
        match self {
            Harness::Agent(agent) => agent.chooses_a_model(),
            Harness::Custom(_) => true,
        }
    }

    fn policy_reaching(&self, platform: Platform, model: &[Endpoint]) -> String {
        match self {
            Harness::Agent(agent) => agent.policy_reaching(platform, model),
            Harness::Custom(custom) => policy_text(
                &custom.name,
                model
                    .iter()
                    .map(|endpoint| (endpoint.host.clone(), endpoint.port)),
                &custom.program,
            ),
        }
    }

    fn env(&self) -> &'static [(&'static str, &'static str)] {
        match self {
            Harness::Agent(agent) => agent.env(),
            Harness::Custom(_) => &[],
        }
    }

    /// What its sandboxes are named after.
    fn name_stem(&self) -> &str {
        match self {
            Harness::Agent(agent) => agent.name_stem(),
            Harness::Custom(custom) => &custom.name,
        }
    }
}

/// The model key, as an agent that reaches whichever service its key is for needs it: the kind of key, the hosts it's
/// sent to, and the model to start with, when the person chose one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub kind: Kind,
    pub endpoints: Vec<Endpoint>,
    pub id: Option<String>,
}

impl Model {
    /// The model key `provider` holds, by the profile OpenShell lists for its type, starting with the model `id` when
    /// one's given. None for a provider Playpen didn't add, whose kind it doesn't know.
    pub fn of(provider: &Provider, profiles: &[Profile], id: Option<String>) -> Option<Model> {
        let kind = Kind::of(&provider.kind)?;
        let endpoints = match profiles.iter().find(|profile| profile.id == provider.kind) {
            Some(profile) => profile
                .endpoints
                .iter()
                .map(|endpoint| Endpoint {
                    host: endpoint.host.clone(),
                    port: endpoint.port,
                })
                .collect(),
            None => kind
                .hosts()
                .iter()
                .map(|host| Endpoint {
                    host: (*host).to_string(),
                    port: 443,
                })
                .collect(),
        };
        let id = id
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty());
        Some(Model {
            kind,
            endpoints,
            id,
        })
    }

    /// The name Pi and its kin know the service by: their own for each they know, and `litellm` for a proxy, which
    /// Playpen describes to them.
    fn provider(&self) -> &'static str {
        match self.kind {
            Kind::Anthropic | Kind::ClaudeSubscription => "anthropic",
            Kind::OpenAi | Kind::ChatGptSubscription => "openai",
            Kind::OpenRouter => "openrouter",
            Kind::Fireworks => "fireworks",
            Kind::ZAi => "zai",
            Kind::LiteLlm => "litellm",
            Kind::Cursor => "cursor",
        }
    }

    /// The model, as Pi and its kin name one: under its service.
    fn qualified(&self) -> Option<String> {
        self.id
            .as_deref()
            .map(|id| format!("{}/{id}", self.provider()))
    }

    /// Where a LiteLLM proxy's OpenAI-style API is. One on 443 is taken as HTTPS, and one on another port as plain
    /// HTTP, as a proxy on a LAN usually is, since OpenShell keeps only its host and port.
    fn base_url(&self) -> Option<String> {
        let endpoint = self.endpoints.first()?;
        Some(match endpoint.port {
            443 => format!("https://{}/v1", endpoint.host),
            80 => format!("http://{}/v1", endpoint.host),
            port => format!("http://{}:{port}/v1", endpoint.host),
        })
    }
}

/// A sandbox to make for an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub name: String,
    pub agent: Harness,
    /// An image to start one of Playpen's agents from instead of the one Playpen builds for it: the person's, which
    /// has the agent where Playpen's does, and node for its settings. A custom agent always names its own.
    pub image: Option<String>,
    /// The provider holding its model's key, by name.
    pub provider: Option<String>,
    /// What the agent needs to know of its key, when it reaches whichever service that's for.
    pub model: Option<Model>,
    /// The ChatGPT account, when the provider is a ChatGPT subscription.
    pub account: Option<Account>,
    /// Services to attach besides the model's key, each with its hosts opened.
    pub services: Vec<Service>,
    /// Variables to set in it: values, and secrets fetched from 1Password or the Keychain as it's made.
    pub variables: Vec<Variable>,
    /// Its kits' setup folders: each `install.sh` run over its image, and the dotfiles linked into its home.
    pub setups: Vec<SetupFolder>,
    /// Folders on the Mac to copy in, for it to work on: none, one, or several, such as the repositories of one
    /// piece of work.
    pub folders: Vec<PathBuf>,
}

/// How far making a sandbox has got, for showing while it waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Building the agent's image, which happens once for each version.
    Building,
    /// Pushing it to the registry a gateway pulls from.
    Pushing,
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
    #[error(transparent)]
    BringBack(#[from] crate::bring_back::BringBackError),
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
    #[error("Say which of the LiteLLM proxy's models {0} starts with: only the proxy knows them.")]
    NoModel(String),
    #[error(
        "Playpen couldn't tell which hosts the model key is for, so {0} couldn't reach its model. Try again once OpenShell lists the gateway's providers."
    )]
    NoModelHosts(String),
    #[error(
        "A kit that installs software is built into an image with this Mac's Docker, which a gateway that pulls from a registry can't use yet. Use a gateway that shares this Mac's Docker, or leave that kit out."
    )]
    KitsNeedLocalDocker,
}

impl Plan {
    /// The image it's made from: its agent's, with its kits' `install.sh`s run over it when they have any.
    pub fn image(&self) -> String {
        image_name(&self.base(), &self.setups)
    }

    /// The image its kits are run over, or that it starts from when there are none.
    fn base(&self) -> Base {
        match (&self.agent, &self.image) {
            (Harness::Agent(agent), None) => Base {
                id: agent.id().to_string(),
                image: agent.image(),
                builds: Some(*agent),
            },
            (Harness::Agent(agent), Some(image)) => Base {
                id: agent.id().to_string(),
                image: image.clone(),
                builds: None,
            },
            (Harness::Custom(custom), _) => Base {
                id: format!("custom-{}", custom.name),
                image: custom.image.clone(),
                builds: None,
            },
        }
    }

    /// Whether it starts from an image named in full, which nothing builds or pushes, so the gateway needn't say where
    /// its images come from.
    pub fn names_its_image(&self) -> bool {
        self.image.is_some() || matches!(self.agent, Harness::Custom(_))
    }

    /// The sandbox's policy: its agent reaching its model from its program in an image for `platform`, and each
    /// service's hosts.
    pub fn policy(&self, platform: Platform) -> String {
        let model = match (&self.model, self.agent.chooses_a_model()) {
            (Some(model), true) => model.endpoints.as_slice(),
            _ => &[],
        };
        let mut policy = self.agent.policy_reaching(platform, model);
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

    /// What follows `openshell sandbox create`, with the policy at `policy`, from the image `image` its source gave.
    /// The sandbox starts with no command and stays up, so its agent can be opened, closed and opened again; the folder
    /// is copied in after, since OpenShell can't copy before a command it's given.
    pub fn create_args(&self, policy: &Path, image: &str) -> Vec<String> {
        let mut args: Vec<String> = [
            "--name",
            &self.name,
            "--from",
            image,
            "--policy",
            &policy.to_string_lossy(),
            "--detach",
            // Never a prompt, since nobody sees one, and nothing it wasn't given.
            "--no-auto-providers",
            // Blocked connections become proposals that wait for the person, never rules on their own.
            "--approval-mode",
            "manual",
            "--label",
            &format!("{AGENT_LABEL}={}", self.agent.label()),
            "--annotation",
            &format!("{WORKDIR_ANNOTATION}={}", self.workdir()),
        ]
        .map(String::from)
        .into();
        // A custom agent's sandbox carries its own name and commands, so it can be opened however the agent is later
        // changed, or if it's deleted.
        if let Harness::Custom(custom) = &self.agent {
            args.extend([
                "--annotation".to_string(),
                format!("{CUSTOM_NAME_ANNOTATION}={}", custom.name),
                "--annotation".to_string(),
                format!("{COMMAND_ANNOTATION}={}", custom.command),
            ]);
            if let Some(continuing) = &custom.continue_command {
                args.extend([
                    "--annotation".to_string(),
                    format!("{CONTINUE_ANNOTATION}={continuing}"),
                ]);
            }
        }
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

    /// Makes the sandbox: has the agent's image made ready by `images`, creates the sandbox and copies the folder in,
    /// saying each step to `step` as it starts.
    pub fn make(
        &self,
        openshell: &OpenShell,
        images: &dyn ImageSource,
        step: impl Fn(Step),
    ) -> Result<(), MakeError> {
        if !is_sandbox_name(&self.name) {
            return Err(MakeError::Name(self.name.clone()));
        }
        if let Some(name) = self.shared_folder_name() {
            return Err(MakeError::SameFolderName(name));
        }
        if self.agent.chooses_a_model()
            && let Some(model) = &self.model
        {
            if model.kind == Kind::LiteLlm && model.id.is_none() {
                return Err(MakeError::NoModel(self.agent.name().to_string()));
            }
            if model.endpoints.is_empty() {
                return Err(MakeError::NoModelHosts(self.agent.name().to_string()));
            }
        }
        let image = if self.setups.iter().any(SetupFolder::installs) {
            // A kit's `install.sh` is run over the agent's image by Docker, on this Mac, and the image that makes is
            // the one the sandbox starts from, so only a gateway that shares this Mac's Docker can use it.
            let docker = images.docker().ok_or(MakeError::KitsNeedLocalDocker)?;
            let image = self.image();
            if !docker.has_image(&image)? {
                step(Step::Building);
                crate::kit_setup::build(&self.base(), &self.setups, docker)?;
            }
            image
        } else {
            match self.base() {
                Base {
                    builds: Some(agent),
                    ..
                } => images.ready(&agent.build(), &step)?,
                // Named in full, for the gateway to pull.
                Base { image, .. } => image,
            }
        };
        step(Step::Creating);
        let policy = std::env::temp_dir().join(format!(
            "playpen-policy-{}-{}.yaml",
            std::process::id(),
            self.name
        ));
        std::fs::write(&policy, self.policy(images.platform()))
            .map_err(|error| MakeError::Policy(error.to_string()))?;
        let secrets: Vec<(String, Secret)> = self
            .variables
            .iter()
            .filter_map(|variable| Some((variable.name.clone(), variable.secret()?)))
            .collect();
        let created = openshell.create_with_secrets(
            &self.name,
            &self.create_args(&policy, &image),
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
        // A repository goes in with its history, so its work can come back as a branch; any other folder as files.
        for folder in &self.folders {
            let copied = match Repo::of(folder) {
                Some(repo) => {
                    crate::bring_back::seed(openshell, &self.name, &repo).map_err(MakeError::from)
                }
                None => openshell
                    .upload(&self.name, folder, SANDBOX_HOME)
                    .map_err(MakeError::from),
            };
            if let Err(error) = copied {
                let _ = openshell.delete(&self.name);
                return Err(error);
            }
        }
        if !self.setups.is_empty() {
            step(Step::Preparing);
            if let Err(error) = link_dotfiles(openshell, &self.name, &self.setups) {
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
        let files = self.settings_files();
        if !files.is_empty() {
            step(Step::Preparing);
            let input = serde_json::Value::from(
                files
                    .iter()
                    .map(|(path, contents)| serde_json::json!({ "path": path, "contents": contents }))
                    .collect::<Vec<_>>(),
            )
            .to_string();
            if let Err(error) = openshell.exec_with_input(
                &self.name,
                &format!("set up {} in “{}”", self.agent.name(), self.name),
                &["node", "-e", WRITE_FILES_SCRIPT],
                input.as_bytes(),
            ) {
                let _ = openshell.delete(&self.name);
                return Err(error.into());
            }
        }
        Ok(())
    }

    /// The files Pi and its kin read their settings from, by path in the sandbox's home: the service and model to
    /// start with, a LiteLLM proxy described, and nothing checked for at startup, which the policy would deny. PiG
    /// asks its first-run questions unless it finds settings, so it always has some.
    pub fn settings_files(&self) -> Vec<(String, String)> {
        let model = self.model.as_ref();
        let litellm = model.filter(|model| model.kind == Kind::LiteLlm);
        let Harness::Agent(agent) = &self.agent else {
            return Vec::new();
        };
        match *agent {
            Agent::ClaudeCode | Agent::Codex => Vec::new(),
            // Its agent over HTTP/1.1, which goes through OpenShell's proxy where HTTP/2 streams may not, and no
            // updates, which the sandbox would deny. Cursor keeps whatever else it sets beside these.
            Agent::Cursor => vec![(
                "/sandbox/.cursor/cli-config.json".to_string(),
                pretty(&serde_json::json!({
                    "channel": "static",
                    "network": { "useHttp1ForAgent": true },
                    "version": 1,
                })),
            )],
            Agent::OpenCode => {
                // In the order serde_json sorts them, so the file's the same with or without its `preserve_order`.
                let mut config = serde_json::Map::new();
                config.insert("$schema".into(), "https://opencode.ai/config.json".into());
                config.insert("autoupdate".into(), false.into());
                if let Some(qualified) = model.and_then(Model::qualified) {
                    config.insert("model".into(), qualified.into());
                }
                if let Some(model) = litellm {
                    let id = model.id.clone().unwrap_or_default();
                    config.insert(
                        "provider".into(),
                        serde_json::json!({
                            "litellm": {
                                "models": { id.clone(): { "name": id } },
                                "name": "LiteLLM",
                                "npm": "@ai-sdk/openai-compatible",
                                "options": {
                                    "apiKey": format!("{{env:{}}}", Kind::LiteLlm.variable()),
                                    "baseURL": model.base_url(),
                                },
                            }
                        }),
                    );
                }
                config.insert("share".into(), "disabled".into());
                vec![(
                    "/sandbox/.config/opencode/opencode.json".to_string(),
                    pretty(&serde_json::Value::Object(config)),
                )]
            }
            Agent::Pi | Agent::Pig => {
                let home = if *agent == Agent::Pi {
                    "/sandbox/.pi/agent"
                } else {
                    "/sandbox/.pig/agent"
                };
                // In the order serde_json sorts them, so the file's the same with or without its `preserve_order`.
                let mut settings = serde_json::Map::new();
                if let Some(id) = model.and_then(|model| model.id.clone()) {
                    settings.insert("defaultModel".into(), id.into());
                }
                if let Some(model) = model {
                    settings.insert("defaultProvider".into(), model.provider().into());
                }
                settings.insert("enableInstallTelemetry".into(), false.into());
                let settings = serde_json::Value::Object(settings);
                let mut files = vec![(format!("{home}/settings.json"), pretty(&settings))];
                if let Some(model) = litellm {
                    files.push((
                        format!("{home}/models.json"),
                        pretty(&litellm_models(model)),
                    ));
                }
                files
            }
            Agent::Omp => {
                let home = "/sandbox/.omp/agent";
                let mut config = String::from(
                    "# Written by Playpen: nothing is checked for or updated at startup, which the sandbox would deny.\n\
                     startup:\n  checkUpdate: false\n\
                     update:\n  auto: false\n\
                     marketplace:\n  autoUpdate: off\n\
                     dev:\n  autoqa: false\n",
                );
                if let Some(qualified) = model.and_then(Model::qualified) {
                    config.push_str(&format!("modelRoles:\n  default: {qualified}\n"));
                }
                let mut files = vec![(format!("{home}/config.yml"), config)];
                if let Some(model) = litellm
                    && let Some(base_url) = model.base_url()
                {
                    let id = model.id.as_deref().unwrap_or_default();
                    files.push((
                        format!("{home}/models.yml"),
                        format!(
                            "# Written by Playpen: the LiteLLM proxy this sandbox's key is for.\n\
                             providers:\n  litellm:\n    baseUrl: {base_url}\n    api: openai-completions\n    \
                             apiKey: {variable}\n    models:\n      - id: {id}\n",
                            variable = Kind::LiteLlm.variable(),
                        ),
                    ));
                }
                files
            }
        }
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
            Harness::Custom(_)
            | Harness::Agent(
                Agent::ClaudeCode
                | Agent::Pi
                | Agent::Omp
                | Agent::Pig
                | Agent::OpenCode
                | Agent::Cursor,
            ) => None,
            Harness::Agent(Agent::Codex) => Some(serde_json::json!({
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
/// Writes each file it's given on its input, `[{path, contents}]`, making its folder, readable only by the sandbox's
/// user.
const WRITE_FILES_SCRIPT: &str = r#"
const fs = require("fs");
const path = require("path");
for (const file of JSON.parse(fs.readFileSync(0, "utf8"))) {
  fs.mkdirSync(path.dirname(file.path), { recursive: true, mode: 0o700 });
  fs.writeFileSync(file.path, file.contents, { mode: 0o600 });
}
"#;

/// A LiteLLM proxy described to Pi and PiG, its key read from the variable OpenShell sets.
fn litellm_models(model: &Model) -> serde_json::Value {
    serde_json::json!({
        "providers": {
            "litellm": {
                "api": "openai-completions",
                "apiKey": format!("${}", Kind::LiteLlm.variable()),
                "baseUrl": model.base_url(),
                "models": model.id.iter().map(|id| serde_json::json!({ "id": id })).collect::<Vec<_>>(),
            }
        }
    })
}

fn pretty(value: &serde_json::Value) -> String {
    let mut text = serde_json::to_string_pretty(value).unwrap_or_default();
    text.push('\n');
    text
}

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
pub fn suggested_name(agent: &Harness, folders: &[PathBuf]) -> String {
    let mut name = agent.name_stem().to_string();
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
            if name == agent.name_stem() {
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
pub fn open_command(openshell: &Path, gateway: &Gateway, sandbox: &Sandbox, what: What) -> String {
    let openshell = match gateway.name() {
        // `env` sets it for any shell, since the line may run in fish, where `NAME=value command` is newer.
        Some(gateway) => format!(
            "env {GATEWAY_VARIABLE}={} {}",
            quote(gateway),
            quote(&openshell.to_string_lossy())
        ),
        None => quote(&openshell.to_string_lossy()),
    };
    let name = quote(&sandbox.name);
    let workdir = quote(sandbox.workdir.as_deref().unwrap_or(SANDBOX_HOME));
    // A custom agent's commands come from the sandbox, which anyone on a shared gateway can change, so they're given to
    // the sandbox's shell whole and quoted, never to the Mac's.
    if let Some(custom) = &sandbox.custom {
        let run = match what {
            What::Agent => Some(custom.command.as_str()),
            What::Continue => custom.continue_command.as_deref(),
            What::Shell => None,
        }
        .map_or_else(
            || "bash -l".to_string(),
            |run| format!("sh -c {}", quote(run)),
        );
        return format!(
            "{openshell} sandbox exec --name {name} --tty --workdir {workdir} -- {run}"
        );
    }
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
    format!("{openshell} sandbox exec --name {name} --tty --workdir {workdir} -- {run}")
}

/// A short hash of `bytes` that's the same in every build, unlike the standard library's, so an image's tag only
/// changes when what it's built from does.
fn fnv1a(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
    })
}

/// `text` as one word to a POSIX shell.
pub(crate) fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Agent, Model, Plan, is_sandbox_name, open_command, suggested_name};
    use crate::image::Platform;
    use crate::openshell::{Gateway, Phase, Sandbox};
    use crate::provider::{Endpoint, Kind};
    use crate::session::What;

    fn plan() -> Plan {
        Plan {
            name: "claude-patchbay".to_string(),
            agent: Agent::ClaudeCode.into(),
            image: None,
            provider: Some("claude-sub".to_string()),
            model: None,
            account: None,
            services: Vec::new(),
            variables: Vec::new(),
            setups: Vec::new(),
            folders: vec![PathBuf::from("/Users/sam/src/Patchbay")],
        }
    }

    #[test]
    fn claude_codes_policy_allows_its_model_from_its_real_binary_and_nothing_else() {
        insta::assert_snapshot!(Agent::ClaudeCode.policy(Platform::host()));
    }

    #[test]
    fn codexs_policy_allows_chatgpt_and_openai_from_its_real_binary() {
        let policy = Agent::Codex.policy(Platform::host());
        assert!(policy.contains("host: chatgpt.com") && policy.contains("host: api.openai.com"));
        assert!(
            policy.contains("/bin/codex\n"),
            "the binary, not codex.js: {policy}"
        );
    }

    #[test]
    fn codexs_binary_is_the_one_for_the_platform_its_image_is_built_for() {
        let arm = Agent::Codex.policy(Platform::Arm64);
        let amd = Agent::Codex.policy(Platform::Amd64);
        assert!(arm.contains("codex-linux-arm64/vendor/aarch64-unknown-linux-musl/bin/codex"));
        assert!(amd.contains("codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex"));
        assert_eq!(
            Agent::ClaudeCode.policy(Platform::Arm64),
            Agent::ClaudeCode.policy(Platform::Amd64),
            "Claude Code's binary is the same on both"
        );
    }

    #[test]
    fn codex_on_a_chatgpt_subscription_is_set_up_with_the_account_and_its_sandbox_off() {
        let codex = Plan {
            agent: Agent::Codex.into(),
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
        let args = plan().create_args(Path::new("/tmp/policy.yaml"), &Agent::ClaudeCode.image());
        insta::assert_snapshot!(args.join("\n"));
    }

    #[test]
    fn a_sandbox_is_started_from_the_image_its_source_named() {
        let from = |image: &str| {
            let args = plan().create_args(Path::new("/tmp/policy.yaml"), image);
            let at = args.iter().position(|arg| arg == "--from").unwrap();
            args[at + 1].clone()
        };
        assert_eq!(
            from("playpen-claude-code:2.1.329"),
            "playpen-claude-code:2.1.329"
        );
        assert_eq!(
            from("registry.example.com/team/playpen-claude-code:2.1.329"),
            "registry.example.com/team/playpen-claude-code:2.1.329"
        );
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
        assert!(
            several
                .create_args(Path::new("/tmp/policy.yaml"), &several.image())
                .contains(
                    &"playpen.folder=/Users/sam/src/Patchbay\n/Users/sam/src/Typecase".to_string()
                )
        );
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
        let named =
            |folder: &str| suggested_name(&Agent::ClaudeCode.into(), &[PathBuf::from(folder)]);
        assert_eq!(named("/src/Patchbay"), "claude-patchbay");
        assert_eq!(named("/src/my app (2)"), "claude-my-app-2");
        assert_eq!(suggested_name(&Agent::ClaudeCode.into(), &[]), "claude");
        let several = |folders: &[&str]| {
            let folders: Vec<PathBuf> = folders.iter().map(PathBuf::from).collect();
            suggested_name(&Agent::ClaudeCode.into(), &folders)
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
            custom: None,
        };
        let openshell = Path::new("/opt/homebrew/bin/openshell");
        let active = Gateway::Active;
        insta::assert_snapshot!(open_command(openshell, &active, &sandbox, What::Agent));
        assert!(
            open_command(openshell, &active, &sandbox, What::Continue)
                .ends_with("-- claude --continue")
        );
        assert!(open_command(openshell, &active, &sandbox, What::Shell).ends_with("-- bash -l"));
        let theirs = Sandbox {
            agent: None,
            ..sandbox
        };
        assert!(
            open_command(openshell, &active, &theirs, What::Continue)
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
            custom: None,
        };
        let openshell = Path::new("/opt/homebrew/bin/openshell");
        let active = Gateway::Active;
        insta::assert_snapshot!(open_command(openshell, &active, &sandbox, What::Continue));
        assert!(open_command(openshell, &active, &sandbox, What::Shell).ends_with("-- bash -l"));
    }

    #[test]
    fn opening_in_a_named_gateway_sets_it_for_the_line_whatever_shell_runs_it() {
        let sandbox = Sandbox {
            name: "claude-patchbay".to_string(),
            phase: Phase::Ready,
            created: String::new(),
            agent: Some("claude-code".to_string()),
            folders: Vec::new(),
            workdir: None,
            custom: None,
        };
        let openshell = Path::new("/opt/homebrew/bin/openshell");
        let line = open_command(
            openshell,
            &Gateway::named("k8s home"),
            &sandbox,
            What::Agent,
        );
        assert!(line.starts_with(
            "env OPENSHELL_GATEWAY='k8s home' '/opt/homebrew/bin/openshell' sandbox exec"
        ));
        let connect = Sandbox {
            agent: None,
            ..sandbox
        };
        assert!(
            open_command(openshell, &Gateway::named("k8s"), &connect, What::Shell).starts_with(
                "env OPENSHELL_GATEWAY='k8s' '/opt/homebrew/bin/openshell' sandbox connect"
            )
        );
    }

    fn pi_plan(agent: Agent, model: Model) -> Plan {
        Plan {
            name: "pi-patchbay".to_string(),
            agent: agent.into(),
            image: None,
            provider: Some("work".to_string()),
            model: Some(model),
            ..plan()
        }
    }

    fn litellm(id: Option<&str>) -> Model {
        Model {
            kind: Kind::LiteLlm,
            endpoints: vec![Endpoint {
                host: "litellm.example.com".to_string(),
                port: 4000,
            }],
            id: id.map(str::to_string),
        }
    }

    fn openrouter() -> Model {
        Model {
            kind: Kind::OpenRouter,
            endpoints: vec![Endpoint {
                host: "openrouter.ai".to_string(),
                port: 443,
            }],
            id: Some("anthropic/claude-sonnet-4.5".to_string()),
        }
    }

    #[test]
    fn pis_policy_reaches_only_the_host_its_key_is_for_from_node() {
        insta::assert_snapshot!(pi_plan(Agent::Pi, litellm(Some("gpt-5"))).policy(Platform::Arm64));
        let omp = pi_plan(Agent::Omp, openrouter()).policy(Platform::Arm64);
        assert!(
            omp.contains("host: openrouter.ai\n        port: 443"),
            "{omp}"
        );
        assert!(omp.contains("path: /usr/local/bin/omp"), "{omp}");
        assert!(
            pi_plan(Agent::Pig, openrouter())
                .policy(Platform::Amd64)
                .contains("path: /usr/local/bin/pig")
        );
    }

    #[test]
    fn claude_codes_policy_is_the_same_whatever_its_key_says() {
        let with = Plan {
            model: Some(openrouter()),
            ..plan()
        };
        assert_eq!(with.policy(Platform::Arm64), plan().policy(Platform::Arm64));
    }

    #[test]
    fn pi_and_its_kin_take_api_keys_and_no_subscription() {
        for agent in [Agent::Pi, Agent::Omp, Agent::Pig] {
            assert!(agent.takes("playpen-openrouter"));
            assert!(agent.takes("playpen-anthropic"));
            assert!(agent.takes("playpen-openai"));
            assert!(agent.takes("playpen-litellm-work"));
            assert!(!agent.takes("claude-code-oauth"), "{agent:?}");
            assert!(!agent.takes("playpen-chatgpt"), "{agent:?}");
            assert!(agent.chooses_a_model());
        }
        assert!(Agent::ClaudeCode.takes("claude-code"));
        assert!(Agent::ClaudeCode.takes("claude-code-oauth"));
        assert!(!Agent::ClaudeCode.takes("playpen-openrouter"));
        assert!(Agent::Codex.takes("playpen-chatgpt"));
        assert!(!Agent::Codex.chooses_a_model());
    }

    #[test]
    fn pi_is_told_its_service_and_model_and_a_litellm_proxy_is_described_to_it() {
        let files = pi_plan(Agent::Pi, litellm(Some("gpt-5"))).settings_files();
        let shown: Vec<String> = files
            .iter()
            .map(|(path, contents)| format!("{path}\n{contents}"))
            .collect();
        insta::assert_snapshot!(shown.join("\n"));
        let pig = pi_plan(Agent::Pig, openrouter()).settings_files();
        assert_eq!(pig.len(), 1, "no proxy to describe");
        assert_eq!(pig[0].0, "/sandbox/.pig/agent/settings.json");
        assert!(pig[0].1.contains("\"defaultProvider\": \"openrouter\""));
    }

    #[test]
    fn omp_is_told_its_model_and_checks_for_nothing_at_startup() {
        let files = pi_plan(Agent::Omp, litellm(Some("gpt-5"))).settings_files();
        let shown: Vec<String> = files
            .iter()
            .map(|(path, contents)| format!("{path}\n{contents}"))
            .collect();
        insta::assert_snapshot!(shown.join("\n"));
    }

    #[test]
    fn a_litellm_proxys_address_is_https_only_on_443() {
        assert_eq!(
            litellm(None).base_url().as_deref(),
            Some("http://litellm.example.com:4000/v1")
        );
        let on_443 = Model {
            endpoints: vec![Endpoint {
                host: "litellm.example.com".to_string(),
                port: 443,
            }],
            ..litellm(None)
        };
        assert_eq!(
            on_443.base_url().as_deref(),
            Some("https://litellm.example.com/v1")
        );
    }

    #[test]
    fn opencode_is_told_its_model_and_a_litellm_proxy_and_checks_for_nothing() {
        let files = pi_plan(Agent::OpenCode, litellm(Some("gpt-5"))).settings_files();
        let shown: Vec<String> = files
            .iter()
            .map(|(path, contents)| format!("{path}\n{contents}"))
            .collect();
        insta::assert_snapshot!(shown.join("\n"));
        assert!(
            pi_plan(Agent::OpenCode, openrouter())
                .policy(Platform::Arm64)
                .contains("host: openrouter.ai")
        );
    }

    #[test]
    fn cursor_reaches_only_cursors_api_from_its_own_node_on_a_cursor_key() {
        let cursor = Plan {
            agent: Agent::Cursor.into(),
            ..plan()
        };
        insta::assert_snapshot!(cursor.policy(Platform::Arm64));
        assert!(Agent::Cursor.takes("playpen-cursor"));
        assert!(!Agent::Cursor.takes("playpen-openrouter"));
        assert!(
            !Agent::Pi.takes("playpen-cursor"),
            "a Cursor key is Cursor's alone"
        );
        assert_eq!(
            cursor.settings_files()[0].0,
            "/sandbox/.cursor/cli-config.json"
        );
    }

    #[test]
    fn claude_code_and_codex_have_no_settings_files_written() {
        assert!(plan().settings_files().is_empty());
    }

    #[test]
    fn an_agent_is_found_by_its_label() {
        assert_eq!(Agent::from_id("claude-code"), Some(Agent::ClaudeCode));
        assert_eq!(Agent::from_id("codex"), Some(Agent::Codex));
        assert_eq!(Agent::from_id("pi"), Some(Agent::Pi));
        assert_eq!(Agent::from_id("omp"), Some(Agent::Omp));
        assert_eq!(Agent::from_id("pig"), Some(Agent::Pig));
        assert_eq!(Agent::from_id("cursor"), Some(Agent::Cursor));
        assert_eq!(Agent::from_id("aider"), None);
    }

    #[test]
    fn an_agents_image_is_tagged_with_its_version_and_its_dockerfile() {
        let tag = Agent::Codex.image();
        assert!(tag.starts_with("playpen-codex:0.171.0-"), "{tag}");
        assert_eq!(tag, Agent::Codex.image(), "the same each time");
        assert_ne!(
            super::fnv1a(b"FROM debian\n"),
            super::fnv1a(b"FROM debian\nRUN true\n"),
            "a changed Dockerfile, a changed tag"
        );
    }
}
