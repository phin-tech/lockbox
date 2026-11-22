//! OpenShell, through its `openshell` command: the sandboxes on its gateway, the providers that hold model keys, and
//! starting, stopping, creating and deleting sandboxes.
//!
//! Playpen asks the command rather than the gateway's gRPC API, since the command already holds the gateway's mTLS
//! certificates and knows which gateway is active. Every call blocks, so the app makes them off its window's thread.

mod services;

pub use services::{Profile, ProfileEndpoint, Service};

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde::Deserialize;

use crate::chatgpt::Tokens;
use crate::new_service::{KeySource, NewService, with_key};
use crate::provider::{CHATGPT_CLIENT_ID, Kind, NewProvider};
use crate::secrets::{Secret, Wrapped, wrap};

mod microvm;
pub use microvm::{
    MICROVM_GATEWAY, MicroVm, MicroVmError, MicroVmPlaces, MicroVmSize, Missing, SetUpStep,
    install_e2fsprogs, why_not_started,
};
mod activity;
pub use activity::{Activity, Reached};
mod ports;
pub use ports::{Forward, Forwards, ListedForward};
mod register;
mod requests;
pub use register::{Forgotten, NewGateway, NewGatewayError, Oidc, Reach};
pub use requests::{NetworkRequest, RequestStatus};

/// The label Playpen puts on a sandbox it made, naming its [`crate::Agent`].
pub const AGENT_LABEL: &str = "playpen.agent";

/// The annotation holding the folders on the Mac that were copied into a sandbox, one a line.
pub const FOLDER_ANNOTATION: &str = "playpen.folder";

/// The annotation holding where the agent works in a sandbox: the copied folder, under `/sandbox`.
pub const WORKDIR_ANNOTATION: &str = "playpen.workdir";

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
    #[error("OpenShell answered in a way Playpen doesn't understand: {0}")]
    Unreadable(String),
    /// A script run in a sandbox ended with this status, which it gives to say something other than failure.
    #[error("The command in the sandbox ended with status {0}")]
    Exit(i32),
}

/// One sandbox, as OpenShell lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sandbox {
    pub name: String,
    pub phase: Phase,
    /// When it was made, as OpenShell writes it: `2026-11-20 03:08:15`, in UTC.
    pub created: String,
    /// The agent Playpen made it for, by the label's value; none for a sandbox made some other way.
    pub agent: Option<String>,
    /// The folders on the Mac copied into it.
    pub folders: Vec<String>,
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

/// Whether adding a provider imported its profile or found it there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Imported {
    New,
    Existing,
}

/// A provider's key, kept out of anything printed or logged.
pub struct Key(pub(crate) String);

impl Key {
    pub fn new(key: &str) -> Key {
        Key(key.trim().to_string())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The key itself, for a masked field of the person's own, as when Playpen fetched it for them.
    pub fn reveal(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Key {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Key(…)")
    }
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
    #[serde(default)]
    next_page_token: String,
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
            folders: listed
                .annotations
                .remove(FOLDER_ANNOTATION)
                .map(|folders| folders.lines().map(str::to_string).collect())
                .unwrap_or_default(),
            workdir: listed.annotations.remove(WORKDIR_ANNOTATION),
            name: listed.name,
        })
        .collect();
    sandboxes.sort_by(|one, other| one.name.cmp(&other.name));
    Ok(sandboxes)
}

/// A page of the providers in what `openshell provider list -o json` printed.
fn parse_provider_page(json: &str) -> Result<ListedProviders, OpenShellError> {
    serde_json::from_str(json).map_err(|error| OpenShellError::Unreadable(error.to_string()))
}

/// The `resource_version` in an exported profile.
fn resource_version(profile: &str) -> Option<u64> {
    profile
        .lines()
        .find_map(|line| line.strip_prefix("resource_version:"))
        .and_then(|version| version.trim().parse().ok())
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

/// What `openshell` reads to choose its gateway, in place of its `--gateway` flag, so that it reaches a command Playpen
/// puts in a shell line as well as one it runs itself.
pub const GATEWAY_VARIABLE: &str = "OPENSHELL_GATEWAY";

/// Which of OpenShell's gateways a command goes to: the one `openshell gateway select` made active, or one by name,
/// as `openshell gateway list` shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Gateway {
    #[default]
    Active,
    Named(String),
}

impl Gateway {
    pub fn named(name: impl Into<String>) -> Gateway {
        Gateway::Named(name.into())
    }

    /// Its name, or none for the active gateway.
    pub fn name(&self) -> Option<&str> {
        match self {
            Gateway::Active => None,
            Gateway::Named(name) => Some(name),
        }
    }
}

/// A gateway OpenShell has registered, as `openshell gateway list` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GatewayInfo {
    pub name: String,
    #[serde(default)]
    pub endpoint: String,
    /// Whether it's the one `openshell gateway select` made active.
    #[serde(default)]
    pub active: bool,
    /// What OpenShell calls it: `local` for one running in Docker on this Mac.
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub is_remote: bool,
    /// How a user is authenticated to it: `mtls`, `plaintext`, or a sign-in.
    #[serde(default)]
    pub auth: String,
}

impl GatewayInfo {
    /// A gateway taken to be on this Mac, for when the gateways couldn't be listed: what every gateway was before
    /// Playpen had several.
    pub fn assumed_local(name: impl Into<String>) -> GatewayInfo {
        GatewayInfo {
            name: name.into(),
            endpoint: String::new(),
            active: true,
            kind: "local".to_string(),
            is_remote: false,
            auth: "mtls".to_string(),
        }
    }

    /// Whether it uses certificates that OpenShell made for a gateway it runs in Docker on this Mac. They stay when the
    /// gateway is forgotten, since the gateway is still running and they're how to reach it again.
    pub fn has_certificates_of_its_own(&self) -> bool {
        self.is_local() && self.auth == "mtls"
    }

    /// The gateway to name in a call to reach it.
    pub fn gateway(&self) -> Gateway {
        Gateway::named(&self.name)
    }

    /// Whether it runs in this Mac's own Docker, so an image built here is one it can start.
    pub fn is_local(&self) -> bool {
        self.kind == "local" && !self.is_remote
    }
}

/// The gateways in what `openshell gateway list -o json` printed, the active one first and then by name.
pub fn parse_gateways(json: &str) -> Result<Vec<GatewayInfo>, OpenShellError> {
    let mut gateways: Vec<GatewayInfo> = serde_json::from_str(json)
        .map_err(|error| OpenShellError::Unreadable(error.to_string()))?;
    gateways.sort_by(|one, other| {
        other
            .active
            .cmp(&one.active)
            .then_with(|| one.name.cmp(&other.name))
    });
    Ok(gateways)
}

/// The `openshell` command, found where Homebrew and its installer put it. An app opened from the Finder has no
/// shell's `PATH`, so it's looked for by its full path first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenShell {
    binary: PathBuf,
    gateway: Gateway,
}

impl OpenShell {
    pub fn new(binary: impl Into<PathBuf>) -> OpenShell {
        OpenShell {
            binary: binary.into(),
            gateway: Gateway::Active,
        }
    }

    /// The same command, sending everything to `gateway`.
    pub fn on(mut self, gateway: Gateway) -> OpenShell {
        self.gateway = gateway;
        self
    }

    pub fn gateway(&self) -> &Gateway {
        &self.gateway
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
        self.run_with(doing, args, None)
    }

    /// Runs `openshell` with `args`, and with `key` in its environment as `variable` when given: a key goes to it
    /// that way, never as an argument, which any process on the Mac can read.
    fn run_with(
        &self,
        doing: &str,
        args: &[&str],
        key: Option<(&str, &Key)>,
    ) -> Result<String, OpenShellError> {
        let output = self.output_with(args, key)?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(OpenShellError::Failed(
                doing.to_string(),
                said(&output.stderr),
            ))
        }
    }

    fn output_with(
        &self,
        args: &[&str],
        key: Option<(&str, &Key)>,
    ) -> Result<Output, OpenShellError> {
        self.command(args, key)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| OpenShellError::Run(error.to_string()))
    }

    /// `openshell` with `args`, set to go to this gateway. A key goes in its environment, never in an argument.
    fn command(&self, args: &[&str], key: Option<(&str, &Key)>) -> Command {
        let mut command = Command::new(&self.binary);
        if let Some((variable, key)) = key {
            command.env(variable, &key.0);
        }
        if let Some(name) = self.gateway.name() {
            command.env(GATEWAY_VARIABLE, name);
        }
        command
            .args(args)
            // Plain text, since its errors are read back and shown.
            .env("NO_COLOR", "1");
        command
    }

    /// A command that runs `openshell` inside `wrapped`, as `op run` or a shell reading the Keychain, still on this
    /// gateway: the wrapper passes its environment on to `openshell`.
    fn wrapped_command(&self, wrapped: &Wrapped) -> Command {
        let mut command = Command::new(&wrapped.program);
        command
            .args(&wrapped.args)
            .envs(wrapped.env.iter().map(|(name, value)| (name, value)));
        if let Some(name) = self.gateway.name() {
            command.env(GATEWAY_VARIABLE, name);
        }
        command.env("NO_COLOR", "1").stdin(Stdio::null());
        command
    }

    /// The gateways OpenShell has registered, whichever this command is set to. It's asked with none named, since
    /// OpenShell marks the one that's named as the active one, and the order would change with the gateway shown.
    pub fn gateways(&self) -> Result<Vec<GatewayInfo>, OpenShellError> {
        parse_gateways(&self.without_gateway().run(
            "list the gateways",
            &["gateway", "list", "--output", "json"],
        )?)
    }

    /// The same command, going to the active gateway.
    fn without_gateway(&self) -> OpenShell {
        self.clone().on(Gateway::Active)
    }

    pub fn sandboxes(&self) -> Result<Vec<Sandbox>, OpenShellError> {
        parse_sandboxes(&self.run(
            "list the sandboxes",
            &["sandbox", "list", "--output", "json"],
        )?)
    }

    /// Every provider, by name, a page at a time as OpenShell gives them.
    pub fn providers(&self) -> Result<Vec<Provider>, OpenShellError> {
        let mut providers = Vec::new();
        let mut token = String::new();
        loop {
            let page = parse_provider_page(&self.run(
                "list the providers",
                &[
                    "provider",
                    "list",
                    "--output",
                    "json",
                    "--page-token",
                    &token,
                ],
            )?)?;
            providers.extend(page.providers);
            if page.next_page_token.is_empty() || page.next_page_token == token {
                break;
            }
            token = page.next_page_token;
        }
        providers.sort_by(|one, other| one.name.cmp(&other.name));
        Ok(providers)
    }

    /// Adds a provider holding `key`, first importing the profile its type comes from. A profile Playpen wrote is
    /// brought up to date when it's there already; `claude-code-oauth`, which providers had before Playpen wrote
    /// profiles, is left as it is. A name already taken is refused before any profile is touched, so adding a
    /// LiteLLM proxy again can't point another provider's key somewhere else.
    pub fn add_provider(&self, new: &NewProvider, key: &Key) -> Result<(), OpenShellError> {
        let adding = format!("add “{}”", new.name);
        if self
            .providers()?
            .iter()
            .any(|provider| provider.name == new.name)
        {
            return Err(OpenShellError::Failed(
                adding,
                "there's a provider called that already".to_string(),
            ));
        }
        let id = new.profile_id();
        let imported = self.import_profile(&new.name, &id, &new.profile())?;
        let variable = new.kind.variable();
        let created = self.run_with(
            &adding,
            &[
                "provider",
                "create",
                "--name",
                &new.name,
                "--type",
                &id,
                "--credential",
                variable,
            ],
            Some((variable, key)),
        );
        if created.is_err() && imported == Imported::New && new.kind == Kind::LiteLlm {
            // A proxy's profile is its provider's alone, so it shouldn't outlive a provider that wasn't made.
            let _ = self.run(&adding, &["provider", "profile", "delete", &id, "--global"]);
        }
        created.map(drop)
    }

    /// Puts `provider`, as it is on the gateway `from`, onto this one: its profile copied across, then the provider
    /// made of it with its key from `source`. OpenShell never gives a key back, so the key has to come from where it
    /// lives, or from the person. A ChatGPT login isn't copied: each gateway renews a login of its own.
    pub fn copy_provider(
        &self,
        from: &OpenShell,
        provider: &Provider,
        source: &KeySource,
        op: Option<&Path>,
    ) -> Result<(), OpenShellError> {
        let adding = format!("add “{}”", provider.name);
        if provider.playpen_kind() == Some(Kind::ChatGptSubscription) {
            return Err(OpenShellError::Failed(
                adding,
                "a ChatGPT login is signed in on each gateway, since each renews its own"
                    .to_string(),
            ));
        }
        if self
            .providers()?
            .iter()
            .any(|there| there.name == provider.name)
        {
            return Err(OpenShellError::Failed(
                adding,
                "this gateway has a provider called that already".to_string(),
            ));
        }
        let profile = from.run(
            &format!("read the profile of “{}”", provider.name),
            &["provider", "profile", "export", &provider.kind, "--global"],
        )?;
        // A profile's version is its gateway's own count, which another gateway doesn't take.
        let profile: String = profile
            .lines()
            .filter(|line| !line.starts_with("resource_version:"))
            .map(|line| format!("{line}\n"))
            .collect();
        let variable = from
            .profiles()?
            .into_iter()
            .find(|profile| profile.id == provider.kind)
            .and_then(|profile| profile.credentials.into_iter().next())
            .and_then(|credential| credential.env_vars.into_iter().next())
            .ok_or_else(|| {
                OpenShellError::Unreadable(format!(
                    "the profile {} names no variable",
                    provider.kind
                ))
            })?;
        self.create_with(
            &adding,
            &provider.name,
            &provider.kind,
            &profile,
            &variable,
            source,
            op,
            provider.has_own_profile(),
        )
    }

    /// Imports the profile `id`, `profile`, for the provider `name`, bringing one Playpen wrote up to date when it's
    /// there already and leaving any other as it is.
    fn import_profile(
        &self,
        name: &str,
        id: &str,
        profile: &str,
    ) -> Result<Imported, OpenShellError> {
        let file =
            std::env::temp_dir().join(format!("playpen-profile-{}-{id}.yaml", std::process::id()));
        std::fs::write(&file, profile).map_err(|error| {
            OpenShellError::Failed(format!("write the profile for “{name}”"), error.to_string())
        })?;
        let path = file.to_string_lossy().into_owned();
        let doing = format!("add the profile for “{name}”");
        let imported = self.run(
            &doing,
            &["provider", "profile", "import", "--global", "--file", &path],
        );
        let imported = match imported {
            Ok(_) => Ok(Imported::New),
            Err(OpenShellError::Failed(_, said)) if said.contains("already exists") => {
                if id.starts_with("playpen-") {
                    self.update_profile(&doing, id, profile, &file)
                        .map(|()| Imported::Existing)
                } else {
                    Ok(Imported::Existing)
                }
            }
            Err(error) => Err(error),
        };
        let _ = std::fs::remove_file(&file);
        imported
    }

    /// Adds a service, its key from `source`: pasted, or from 1Password through `op`, which hands it to `openshell`
    /// itself. Its profile is its own, so it goes again if the provider isn't made.
    pub fn add_service(
        &self,
        new: &NewService,
        source: &KeySource,
        op: Option<&Path>,
    ) -> Result<(), OpenShellError> {
        let adding = format!("add “{}”", new.name);
        if self
            .providers()?
            .iter()
            .any(|provider| provider.name == new.name)
        {
            return Err(OpenShellError::Failed(
                adding,
                "there's a provider called that already".to_string(),
            ));
        }
        let id = new.profile_id();
        self.create_with(
            &adding,
            &new.name,
            &id,
            &new.profile(),
            &new.variable,
            source,
            op,
            true,
        )
    }

    /// Adds a model key whose key comes from `source`, as [`OpenShell::add_provider`] does with a pasted one.
    pub fn add_provider_from(
        &self,
        new: &NewProvider,
        source: &KeySource,
        op: Option<&Path>,
    ) -> Result<(), OpenShellError> {
        let adding = format!("add “{}”", new.name);
        if self
            .providers()?
            .iter()
            .any(|provider| provider.name == new.name)
        {
            return Err(OpenShellError::Failed(
                adding,
                "there's a provider called that already".to_string(),
            ));
        }
        let own = new.kind == Kind::LiteLlm;
        self.create_with(
            &adding,
            &new.name,
            &new.profile_id(),
            &new.profile(),
            new.kind.variable(),
            source,
            op,
            own,
        )
    }

    /// Imports the profile `id` and makes the provider `name` of it, its key in `variable` from `source`: set here
    /// when pasted, or by `op run` from 1Password. A profile that's the provider's `own` goes again if it isn't made.
    #[expect(
        clippy::too_many_arguments,
        reason = "each is a different part of the one provider"
    )]
    fn create_with(
        &self,
        adding: &str,
        name: &str,
        id: &str,
        profile: &str,
        variable: &str,
        source: &KeySource,
        op: Option<&Path>,
        own: bool,
    ) -> Result<(), OpenShellError> {
        let args = [
            "provider",
            "create",
            "--name",
            name,
            "--type",
            id,
            "--credential",
            variable,
        ];
        let wrapped = with_key(source, variable, op, &self.binary, &args)
            .map_err(|error| OpenShellError::Failed(adding.to_string(), error.to_string()))?;
        let imported = self.import_profile(name, id, profile)?;
        let created = self
            .wrapped_command(&wrapped)
            .output()
            .map_err(|error| OpenShellError::Run(error.to_string()))
            .and_then(|output| {
                if output.status.success() {
                    Ok(())
                } else {
                    Err(OpenShellError::Failed(
                        adding.to_string(),
                        said(&output.stderr),
                    ))
                }
            });
        if created.is_err() && own && imported == Imported::New {
            let _ = self.run(adding, &["provider", "profile", "delete", id, "--global"]);
        }
        created
    }

    /// Replaces the profile `id` with `profile`, written to `file`. OpenShell takes an update only with the version it
    /// has, so that's read from the profile as it stands.
    fn update_profile(
        &self,
        doing: &str,
        id: &str,
        profile: &str,
        file: &Path,
    ) -> Result<(), OpenShellError> {
        let current = self.run(doing, &["provider", "profile", "export", id, "--global"])?;
        let version = resource_version(&current).ok_or_else(|| {
            OpenShellError::Unreadable(format!("the profile {id} has no version"))
        })?;
        std::fs::write(file, format!("resource_version: {version}\n{profile}"))
            .map_err(|error| OpenShellError::Failed(doing.to_string(), error.to_string()))?;
        let path = file.to_string_lossy();
        self.run(
            doing,
            &[
                "provider", "profile", "update", id, "--global", "--file", &path,
            ],
        )
        .map(drop)
    }

    /// Gives `provider` a new key. Sandboxes it's attached to take it up for the processes they start next.
    pub fn replace_key(&self, provider: &Provider, key: &Key) -> Result<(), OpenShellError> {
        let variable = provider.playpen_kind().map(Kind::variable).ok_or_else(|| {
            OpenShellError::Failed(
                format!("replace the key for “{}”", provider.name),
                "Playpen doesn't know its kind".to_string(),
            )
        })?;
        self.run_with(
            &format!("replace the key for “{}”", provider.name),
            &[
                "provider",
                "update",
                &provider.name,
                "--credential",
                variable,
            ],
            Some((variable, key)),
        )
        .map(drop)
    }

    /// Adds a ChatGPT provider holding a sign-in's access token, and has OpenShell renew it with the sign-in's refresh
    /// token. A provider that can't be renewed would stop working within days, so it's deleted again if that fails.
    pub fn add_signed_in(&self, new: &NewProvider, tokens: &Tokens) -> Result<(), OpenShellError> {
        self.add_provider(new, &tokens.access)?;
        if let Err(error) = self.renew_with(&new.name, tokens) {
            let _ = self.run(&error.to_string(), &["provider", "delete", &new.name]);
            return Err(error);
        }
        Ok(())
    }

    /// Gives a ChatGPT provider a new sign-in. OpenShell won't take a new value for a credential it renews, so the
    /// renewal is cleared first and set up again after.
    pub fn sign_in_again(
        &self,
        provider: &Provider,
        tokens: &Tokens,
    ) -> Result<(), OpenShellError> {
        self.run(
            &format!("sign “{}” in again", provider.name),
            &[
                "provider",
                "refresh",
                "delete",
                &provider.name,
                "--credential-key",
                Kind::ChatGptSubscription.variable(),
            ],
        )?;
        self.replace_key(provider, &tokens.access)?;
        self.renew_with(&provider.name, tokens)
    }

    /// Has OpenShell renew the ChatGPT provider `name`'s access token with `tokens`' refresh token, as Codex itself
    /// would, a day before it expires. The refresh token reaches `openshell` through its environment.
    fn renew_with(&self, name: &str, tokens: &Tokens) -> Result<(), OpenShellError> {
        const VARIABLE: &str = "PLAYPEN_REFRESH_TOKEN";
        let client = format!("client_id={CHATGPT_CLIENT_ID}");
        let refresh = format!("refresh_token={VARIABLE}");
        let expires = tokens.expires_at_ms.to_string();
        self.run_with(
            &format!("set up renewing “{name}”"),
            &[
                "provider",
                "refresh",
                "configure",
                name,
                "--credential-key",
                Kind::ChatGptSubscription.variable(),
                "--strategy",
                "oauth2-refresh-token",
                "--material",
                &client,
                "--secret-material-env",
                &refresh,
                "--credential-expires-at",
                &expires,
            ],
            Some((VARIABLE, &tokens.refresh)),
        )
        .map(drop)
    }

    /// Runs the shell `script` in the sandbox `name` with `args`, without a terminal or the login profile, whose
    /// output could mix with the script's, and gives the bytes it printed exactly. A status of 3 to 9 is the script's
    /// own answer, given back as [`OpenShellError::Exit`]; anything else that isn't 0 is a failure.
    pub fn exec_bytes(
        &self,
        name: &str,
        doing: &str,
        script: &str,
        args: &[&str],
    ) -> Result<Vec<u8>, OpenShellError> {
        use std::io::Write as _;

        let mut all = vec![
            "sandbox",
            "exec",
            "--name",
            name,
            "--no-tty",
            "--no-login-shell",
            "--",
            "sh",
            "-s",
            "--",
        ];
        all.extend_from_slice(args);
        let mut child = self
            .exec_command(&all)
            .spawn()
            .map_err(|error| OpenShellError::Run(error.to_string()))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(script.as_bytes())
                .map_err(|error| OpenShellError::Failed(doing.to_string(), error.to_string()))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|error| OpenShellError::Run(error.to_string()))?;
        match output.status.code() {
            Some(0) => Ok(output.stdout),
            Some(code @ 3..=9) => Err(OpenShellError::Exit(code)),
            _ => Err(OpenShellError::Failed(
                doing.to_string(),
                said(&output.stderr),
            )),
        }
    }

    /// `openshell` with `args`, on this gateway, with its standard streams piped: a command run in a sandbox, which
    /// is only found on the gateway that holds it.
    fn exec_command(&self, args: &[&str]) -> Command {
        let mut command = self.command(args, None);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// Runs `command` in the sandbox `name`, in its home, with `input` on its standard input, and gives what it
    /// printed. What a sandbox needs that holds anything private goes in this way, never as an argument.
    pub fn exec_with_input(
        &self,
        name: &str,
        doing: &str,
        command: &[&str],
        input: &[u8],
    ) -> Result<String, OpenShellError> {
        use std::io::Write as _;

        let mut args = vec!["sandbox", "exec", "--name", name, "--"];
        args.extend_from_slice(command);
        let mut child = self
            .exec_command(&args)
            .spawn()
            .map_err(|error| OpenShellError::Run(error.to_string()))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(input)
                .map_err(|error| OpenShellError::Failed(doing.to_string(), error.to_string()))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|error| OpenShellError::Run(error.to_string()))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            let said = match said(&output.stderr) {
                said if said == "it gave no reason" => {
                    String::from_utf8_lossy(&output.stdout).trim().to_string()
                }
                said => said,
            };
            Err(OpenShellError::Failed(doing.to_string(), said))
        }
    }

    /// Deletes `provider`, and the profile Playpen made for it alone. OpenShell won't while a sandbox uses it.
    pub fn delete_provider(&self, provider: &Provider) -> Result<(), OpenShellError> {
        self.run(
            &format!("delete “{}”", provider.name),
            &["provider", "delete", &provider.name],
        )?;
        if provider.has_own_profile() {
            self.run(
                &format!("delete the profile for “{}”", provider.name),
                &["provider", "profile", "delete", &provider.kind, "--global"],
            )?;
        }
        Ok(())
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

    /// Makes a sandbox with `args` after `sandbox create`, with `secrets` fetched into `openshell`'s environment for
    /// its `--env-from`s, from 1Password or the Keychain, never through Playpen.
    pub fn create_with_secrets(
        &self,
        name: &str,
        args: &[String],
        secrets: &[(String, Secret)],
        op: Option<&Path>,
    ) -> Result<(), OpenShellError> {
        if secrets.is_empty() {
            return self.create(name, args);
        }
        let doing = format!("make “{name}”");
        let mut command = vec![
            self.binary.to_string_lossy().into_owned(),
            "sandbox".to_string(),
            "create".to_string(),
        ];
        command.extend(args.iter().cloned());
        let wrapped = wrap(command, secrets, None, op)
            .map_err(|error| OpenShellError::Failed(doing.clone(), error.to_string()))?;
        let output = self
            .wrapped_command(&wrapped)
            .output()
            .map_err(|error| OpenShellError::Run(error.to_string()))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(OpenShellError::Failed(doing, said(&output.stderr)))
        }
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
    use std::ffi::OsStr;

    use super::{
        GATEWAY_VARIABLE, Gateway, OpenShell, Phase, parse_gateways, parse_provider_page,
        parse_sandboxes, resource_version, said,
    };

    /// What OpenShell 0.1.8 listed with the gateway it set up on this Mac.
    const GATEWAYS: &str = include_str!("gateways.json");

    #[test]
    fn the_gateway_openshell_set_up_on_this_mac_is_local_and_active() {
        let gateways = parse_gateways(GATEWAYS).unwrap();
        assert_eq!(gateways.len(), 1);
        assert_eq!(gateways[0].name, "openshell");
        assert!(gateways[0].active && gateways[0].is_local());
        assert_eq!(gateways[0].gateway(), Gateway::named("openshell"));
    }

    #[test]
    fn the_active_gateway_is_listed_first_and_a_remote_one_is_not_local() {
        // Written for this test: only the local gateway's shape has been seen.
        let json = r#"[
            {"name": "k8s-home", "endpoint": "https://os.example.com", "active": false, "type": "remote", "is_remote": true},
            {"name": "openshell", "endpoint": "https://localhost:17670", "active": true, "type": "local", "is_remote": false},
            {"name": "ec2", "active": false}
        ]"#;
        let gateways = parse_gateways(json).unwrap();
        let names: Vec<_> = gateways
            .iter()
            .map(|gateway| gateway.name.as_str())
            .collect();
        assert_eq!(names, ["openshell", "ec2", "k8s-home"]);
        assert!(!gateways[2].is_local());
        assert!(
            !gateways[1].is_local(),
            "a gateway that doesn't say isn't assumed to be"
        );
    }

    fn gateway_env(openshell: &OpenShell) -> Option<String> {
        let command = openshell.command(&["sandbox", "list"], None);
        command
            .get_envs()
            .find(|(name, _)| *name == OsStr::new(GATEWAY_VARIABLE))
            .and_then(|(_, value)| value.map(|value| value.to_string_lossy().into_owned()))
    }

    #[test]
    fn commands_go_to_the_active_gateway_unless_one_is_named() {
        let openshell = OpenShell::new("/opt/homebrew/bin/openshell");
        assert_eq!(openshell.gateway(), &Gateway::Active);
        assert_eq!(gateway_env(&openshell), None);
        let named = openshell.on(Gateway::named("k8s-home"));
        assert_eq!(named.gateway().name(), Some("k8s-home"));
        assert_eq!(gateway_env(&named).as_deref(), Some("k8s-home"));
    }

    #[test]
    fn a_command_run_in_a_sandbox_goes_to_the_gateway_that_holds_it() {
        let openshell = OpenShell::new("/opt/homebrew/bin/openshell").on(Gateway::named("microvm"));
        let command = openshell.exec_command(&["sandbox", "exec", "--name", "codex", "--", "true"]);
        let gateway = command
            .get_envs()
            .find(|(name, _)| *name == OsStr::new(GATEWAY_VARIABLE))
            .and_then(|(_, value)| value.map(|value| value.to_string_lossy().into_owned()));
        assert_eq!(gateway.as_deref(), Some("microvm"));
    }

    #[test]
    fn a_command_wrapped_to_fetch_a_key_still_goes_to_the_gateway_named() {
        let wrapped = crate::secrets::Wrapped {
            program: "/opt/homebrew/bin/op".into(),
            args: vec!["run".to_string(), "--".to_string()],
            env: vec![("KEY".to_string(), "op://vault/item/field".to_string())],
        };
        let env = |openshell: &OpenShell| {
            openshell
                .wrapped_command(&wrapped)
                .get_envs()
                .find(|(name, _)| *name == OsStr::new(GATEWAY_VARIABLE))
                .and_then(|(_, value)| value.map(|value| value.to_string_lossy().into_owned()))
        };
        let openshell = OpenShell::new("/opt/homebrew/bin/openshell");
        assert_eq!(env(&openshell), None);
        assert_eq!(
            env(&openshell.on(Gateway::named("kind"))).as_deref(),
            Some("kind")
        );
    }

    #[test]
    fn the_gateways_are_listed_with_none_named_so_the_order_doesnt_follow_the_one_shown() {
        let named = OpenShell::new("/opt/homebrew/bin/openshell").on(Gateway::named("k8s-home"));
        assert_eq!(gateway_env(&named.without_gateway()), None);
        assert_eq!(
            named.gateway().name(),
            Some("k8s-home"),
            "the command itself is as it was"
        );
    }

    /// What OpenShell 0.1.8 listed with one sandbox Playpen made.
    const SANDBOXES: &str = include_str!("sandboxes.json");
    const PROVIDERS: &str = include_str!("providers.json");

    #[test]
    fn a_sandbox_playpen_made_says_which_agent_it_runs() {
        let sandboxes = parse_sandboxes(SANDBOXES).unwrap();
        assert_eq!(sandboxes.len(), 1);
        let sandbox = &sandboxes[0];
        assert_eq!(sandbox.name, "lb-idle");
        assert_eq!(sandbox.phase, Phase::Ready);
        assert_eq!(sandbox.agent.as_deref(), Some("claude-code"));
        assert_eq!(sandbox.created, "2026-11-20 03:08:15");
    }

    #[test]
    fn the_folders_copied_in_are_read_one_a_line() {
        let json = r#"{"sandboxes": [
            {"name": "a", "phase": "Ready", "annotations": {"playpen.folder": "/src/Patchbay\n/src/Typecase"}},
            {"name": "b", "phase": "Ready"}
        ]}"#;
        let sandboxes = parse_sandboxes(json).unwrap();
        assert_eq!(sandboxes[0].folders, ["/src/Patchbay", "/src/Typecase"]);
        assert!(sandboxes[1].folders.is_empty());
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
        let providers = parse_provider_page(PROVIDERS).unwrap().providers;
        assert_eq!(providers[0].name, "claude-sub");
        assert_eq!(providers[0].kind, "claude-code-oauth");
    }

    #[test]
    fn a_profiles_version_is_read_from_its_export() {
        assert_eq!(
            resource_version("id: claude-code-oauth\nresource_version: 3\ndisplay_name: x\n"),
            Some(3)
        );
        assert_eq!(resource_version("id: x\n"), None);
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
