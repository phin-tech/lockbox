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
use crate::provider::{CHATGPT_CLIENT_ID, Kind, NewProvider};

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
pub struct Key(String);

impl Key {
    pub fn new(key: &str) -> Key {
        Key(key.trim().to_string())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
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
        let mut command = Command::new(&self.binary);
        if let Some((variable, key)) = key {
            command.env(variable, &key.0);
        }
        command
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
        let file =
            std::env::temp_dir().join(format!("playpen-profile-{}-{id}.yaml", std::process::id()));
        std::fs::write(&file, new.profile()).map_err(|error| {
            OpenShellError::Failed(
                format!("write the profile for “{}”", new.name),
                error.to_string(),
            )
        })?;
        let path = file.to_string_lossy().into_owned();
        let doing = format!("add the profile for “{}”", new.name);
        let imported = self.run(
            &doing,
            &["provider", "profile", "import", "--global", "--file", &path],
        );
        let imported = match imported {
            Ok(_) => Ok(Imported::New),
            Err(OpenShellError::Failed(_, said)) if said.contains("already exists") => {
                if id.starts_with("playpen-") {
                    self.update_profile(&doing, &id, &new.profile(), &file)
                        .map(|()| Imported::Existing)
                } else {
                    Ok(Imported::Existing)
                }
            }
            Err(error) => Err(error),
        };
        let _ = std::fs::remove_file(&file);
        let imported = imported?;
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
            let _ = self.run(&doing, &["provider", "profile", "delete", &id, "--global"]);
        }
        created.map(drop)
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
        let mut child = Command::new(&self.binary)
            .args(&args)
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
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
    use super::{Phase, parse_provider_page, parse_sandboxes, resource_version, said};

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
