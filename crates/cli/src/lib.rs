//! The `playpen` command. It's the app's own binary: given a command it knows, `main` runs it here and exits
//! before any window opens. Anything else opens the app, since macOS starts apps with arguments of its own.
//!
//! What it prints is for a person, or with `--json` for a program: errors always go to standard error, so the
//! output of a command can be piped without them.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use playpen_core::{
    BUILD, Gateway, GatewayInfo, OpenShell, Phase, Provider, Sandbox, Service, ServiceCatalog,
    ServicePreset, VERSION, version,
};
use serde_json::{Value, json};

/// What a command line asked for, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    action: Action,
    /// `--json`: print JSON, not a table.
    json: bool,
    /// `--gateway <name>`: the OpenShell gateway to go to, in place of the active one.
    gateway: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Version,
    Help,
    ServicesList(String),
    ServicesShow(String),
    ServicesExport(Vec<String>),
    ServicesImport(PathBuf),
    Gateways,
    Sandboxes,
    SandboxStart(String),
    SandboxStop(String),
    SandboxRestart(String),
    SandboxDelete { name: String, yes: bool },
    SandboxServices(String),
    SandboxAttach { name: String, provider: String },
    SandboxDetach { name: String, provider: String },
    Providers,
    Wrong(String),
}

const USAGE: &str = "usage: playpen [command] [--json] [--gateway <name>]

  services [query]                    list or search the services catalog
  services show <id>                  everything the catalog says about one service
  services export [id…]               print shareable services catalog JSON
  services import <file>              load a shared services catalog JSON into Playpen

  gateways                            the OpenShell gateways
  sandboxes                           the sandboxes on a gateway
  sandbox start|stop|restart <name>
  sandbox delete <name> --yes
  sandbox services <name>             the services attached to a sandbox
  sandbox attach <name> <provider>    attach a provider to a sandbox as a service
  sandbox detach <name> <provider>
  providers                           the providers on a gateway

  --json                              print JSON, for a program to read
  --gateway <name>                    the gateway to go to; the active one otherwise
  --version, --help

With no arguments, opens Playpen.";

/// The words that start a command, which a near miss is taken for.
const WORDS: &[&str] = &[
    "version",
    "help",
    "services",
    "catalog",
    "gateways",
    "sandboxes",
    "sandbox",
    "providers",
];

/// The command `args` (without the program's name) ask for, or none, to open the app.
///
/// An argument macOS or the Finder passes (`-psn_…`, `-NSDocumentRevisionsDebugMode`, a file's path) opens the app. A
/// `--` option or a near miss of a command is a mistake, and is told so rather than opening a window.
pub fn parse<S: AsRef<str>>(args: &[S]) -> Option<Command> {
    let args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
    let first = *args.first()?;

    let mut json = false;
    let mut help = false;
    let mut gateway = None;
    let mut rest: Vec<&str> = Vec::new();
    let mut taken = args.iter().copied();
    let mut mistake = None;
    while let Some(arg) = taken.next() {
        match arg {
            "--json" => json = true,
            "--help" | "-h" => help = true,
            "--gateway" => match taken.next() {
                Some(name) if !name.starts_with('-') => gateway = Some(name.to_string()),
                _ => mistake = Some("--gateway needs the name of a gateway".to_string()),
            },
            _ => match arg.strip_prefix("--gateway=") {
                Some(name) if !name.is_empty() => gateway = Some(name.to_string()),
                Some(_) => mistake = Some("--gateway needs the name of a gateway".to_string()),
                None => rest.push(arg),
            },
        }
    }
    let command = |action| {
        Some(Command {
            action,
            json,
            gateway: gateway.clone(),
        })
    };
    if help {
        return command(Action::Help);
    }
    if let Some(mistake) = mistake {
        return command(Action::Wrong(mistake));
    }
    let Some(&word) = rest.first() else {
        // Only options: `--json` alone has nothing to print.
        return command(Action::Wrong(format!("{first} needs a command")));
    };
    if !WORDS.contains(&word) && !matches!(word, "--version" | "-v") {
        if word.starts_with("--") {
            return command(Action::Wrong(format!("unknown option {word}")));
        }
        if word.starts_with('-') || word.contains(['.', '/']) {
            return None;
        }
        return nearest(word).map(|near| Command {
            action: Action::Wrong(format!("unknown command “{word}”; did you mean “{near}”?")),
            json,
            gateway: gateway.clone(),
        });
    }
    let more = &rest[1..];
    // A `-x` that a command doesn't take is refused by name, not taken for a search or a name.
    let takes_yes = word == "sandbox" && more.first() == Some(&"delete");
    if let Some(flag) = more
        .iter()
        .find(|arg| arg.starts_with('-') && arg.len() > 1 && !(takes_yes && is_yes(arg)))
    {
        return command(Action::Wrong(format!("{word} doesn't take {flag}")));
    }
    command(action(word, more))
}

fn is_yes(arg: &str) -> bool {
    matches!(arg, "--yes" | "-y")
}

/// What `word` followed by `more` asks for.
fn action(word: &str, more: &[&str]) -> Action {
    let wrong = |reason: &str| Action::Wrong(reason.to_string());
    match word {
        "--version" | "-v" | "version" if more.is_empty() => Action::Version,
        "--version" | "-v" | "version" => wrong("version takes no arguments"),
        "help" => Action::Help,
        "services" | "catalog" => match more.first().copied() {
            None => Action::ServicesList(String::new()),
            Some("list" | "ls") => Action::ServicesList(more[1..].join(" ")),
            Some("show") => match &more[1..] {
                [id] => Action::ServicesShow((*id).to_string()),
                [] => wrong("give the id of a service to show"),
                _ => wrong("services show takes one service at a time"),
            },
            Some("export") => {
                Action::ServicesExport(more[1..].iter().map(|id| (*id).to_string()).collect())
            }
            Some("import") => match &more[1..] {
                [path] => Action::ServicesImport(PathBuf::from(path)),
                [] => wrong("give the path to the services JSON file"),
                _ => wrong("services import takes one file at a time"),
            },
            Some(_) => Action::ServicesList(more.join(" ")),
        },
        "gateways" if more.is_empty() => Action::Gateways,
        "sandboxes" if more.is_empty() => Action::Sandboxes,
        "providers" if more.is_empty() => Action::Providers,
        "gateways" | "sandboxes" | "providers" => {
            Action::Wrong(format!("{word} takes no arguments"))
        }
        "sandbox" => match more.first().copied() {
            None => Action::Sandboxes,
            Some("list" | "ls") if more.len() == 1 => Action::Sandboxes,
            Some("list" | "ls") => wrong("sandbox list takes no arguments"),
            Some("start") => sandbox_one(&more[1..], Action::SandboxStart),
            Some("stop") => sandbox_one(&more[1..], Action::SandboxStop),
            Some("restart") => sandbox_one(&more[1..], Action::SandboxRestart),
            Some("services") => sandbox_one(&more[1..], Action::SandboxServices),
            Some("delete") => {
                let yes = more[1..].iter().any(|arg| is_yes(arg));
                let names: Vec<&&str> = more[1..].iter().filter(|arg| !is_yes(arg)).collect();
                match names.as_slice() {
                    [name] => Action::SandboxDelete {
                        name: (**name).to_string(),
                        yes,
                    },
                    [] => wrong("give the name of the sandbox to delete"),
                    _ => wrong("sandbox delete takes one sandbox at a time"),
                }
            }
            Some("attach") => match &more[1..] {
                [name, provider] => Action::SandboxAttach {
                    name: (*name).to_string(),
                    provider: (*provider).to_string(),
                },
                _ => wrong("sandbox attach takes a sandbox and a provider"),
            },
            Some("detach") => match &more[1..] {
                [name, provider] => Action::SandboxDetach {
                    name: (*name).to_string(),
                    provider: (*provider).to_string(),
                },
                _ => wrong("sandbox detach takes a sandbox and a provider"),
            },
            Some(other) => Action::Wrong(format!(
                "sandbox doesn't have “{other}”: it's list, start, stop, restart, delete, services, attach or detach"
            )),
        },
        _ => Action::Wrong(format!("unknown command “{word}”")),
    }
}

fn sandbox_one(more: &[&str], make: fn(String) -> Action) -> Action {
    match more {
        [name] => make((*name).to_string()),
        [] => Action::Wrong("give the name of a sandbox".to_string()),
        _ => Action::Wrong("that takes one sandbox at a time".to_string()),
    }
}

/// The command `word` is a near miss of, if any: one or two letters off, as a typo is.
fn nearest(word: &str) -> Option<&'static str> {
    WORDS
        .iter()
        .map(|candidate| (distance(word, candidate), *candidate))
        .filter(|(distance, candidate)| *distance <= if candidate.len() <= 4 { 1 } else { 2 })
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, candidate)| candidate)
}

/// How many single-letter edits turn `one` into `other`.
fn distance(one: &str, other: &str) -> usize {
    let other: Vec<char> = other.chars().collect();
    let mut row: Vec<usize> = (0..=other.len()).collect();
    for (i, a) in one.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, b) in other.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 1)
                .min(row[j] + 1)
                .min(diagonal + usize::from(a != *b));
            diagonal = above;
        }
    }
    row[other.len()]
}

fn data_dir() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PLAYPEN_SETTINGS") {
        return PathBuf::from(path).parent().map(PathBuf::from);
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    Some(home.join("Library/Application Support/tech.phin.playpen"))
}

/// Why a command didn't finish.
enum Fail {
    /// The command line was wrong: exit status 2, with the usage.
    Usage(String),
    /// It was right and didn't work: exit status 1.
    Failed(String),
    Io(io::Error),
}

impl From<io::Error> for Fail {
    fn from(error: io::Error) -> Fail {
        Fail::Io(error)
    }
}

fn failed(error: impl std::fmt::Display) -> Fail {
    Fail::Failed(format!("{error}"))
}

impl Command {
    /// Runs the command, writing what it prints to `out` and what went wrong to `err`, and gives the exit status.
    pub fn run(self, out: &mut impl Write, err: &mut impl Write) -> io::Result<i32> {
        self.run_in(data_dir(), out, err)
    }

    /// The same, with Playpen's settings folder (where the services catalog files are kept) given.
    fn run_in(
        self,
        dir: Option<PathBuf>,
        out: &mut impl Write,
        err: &mut impl Write,
    ) -> io::Result<i32> {
        match self.execute(dir.as_deref(), out, err) {
            Ok(()) => Ok(0),
            Err(Fail::Usage(reason)) => {
                writeln!(err, "playpen: {reason}\n\n{USAGE}")?;
                Ok(2)
            }
            Err(Fail::Failed(reason)) => {
                writeln!(err, "playpen: {reason}")?;
                Ok(1)
            }
            Err(Fail::Io(error)) => Err(error),
        }
    }

    fn openshell(&self) -> Result<OpenShell, Fail> {
        let shell = OpenShell::find().map_err(failed)?;
        Ok(match &self.gateway {
            Some(name) => shell.on(Gateway::named(name)),
            None => shell,
        })
    }

    fn print(&self, out: &mut impl Write, value: &Value) -> io::Result<()> {
        writeln!(
            out,
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_default()
        )
    }

    /// Says a thing was done: a line, or `{"ok": true, …}` for a program.
    fn done(&self, out: &mut impl Write, what: &str, details: Value) -> io::Result<()> {
        if self.json {
            let mut done = json!({ "ok": true });
            if let (Some(done), Some(details)) = (done.as_object_mut(), details.as_object()) {
                done.extend(details.clone());
            }
            self.print(out, &done)
        } else {
            writeln!(out, "{what}")
        }
    }

    fn execute(
        &self,
        dir: Option<&Path>,
        out: &mut impl Write,
        err: &mut impl Write,
    ) -> Result<(), Fail> {
        match &self.action {
            Action::Version => {
                if self.json {
                    self.print(out, &json!({ "version": VERSION, "build": BUILD }))?;
                } else {
                    writeln!(out, "playpen {}", version())?;
                }
            }
            Action::Help => writeln!(out, "{USAGE}")?,
            Action::Wrong(reason) => return Err(Fail::Usage(reason.clone())),
            Action::ServicesList(query) => {
                let (catalog, errors) = ServiceCatalog::load(dir);
                for error in errors {
                    writeln!(err, "warning: {error}")?;
                }
                let shown = catalog.filter(query, None);
                if self.json {
                    let all: Vec<Value> =
                        shown.iter().map(|service| preset_json(service)).collect();
                    self.print(out, &Value::Array(all))?;
                } else if shown.is_empty() {
                    writeln!(err, "No services match “{query}”.")?;
                } else {
                    let rows = shown
                        .iter()
                        .map(|service| {
                            vec![
                                service.id.clone(),
                                service.resolved_host(),
                                service.variable.clone(),
                                service.summary.clone(),
                            ]
                        })
                        .collect();
                    table(out, &["ID", "HOST", "VARIABLE", "SUMMARY"], rows)?;
                }
            }
            Action::ServicesShow(id) => {
                let (catalog, errors) = ServiceCatalog::load(dir);
                for error in errors {
                    writeln!(err, "warning: {error}")?;
                }
                let Some(service) = catalog.find(id) else {
                    return Err(Fail::Failed(format!(
                        "there's no service “{id}”; `playpen services` lists them"
                    )));
                };
                if self.json {
                    self.print(out, &preset_json(service))?;
                } else {
                    show(out, service)?;
                }
            }
            Action::ServicesExport(ids) => {
                let (catalog, errors) = ServiceCatalog::load(dir);
                for error in errors {
                    writeln!(err, "warning: {error}")?;
                }
                let missing: Vec<&str> = ids
                    .iter()
                    .filter(|id| catalog.find(id).is_none())
                    .map(String::as_str)
                    .collect();
                if !missing.is_empty() {
                    return Err(Fail::Failed(format!(
                        "there's no service {}; `playpen services` lists them",
                        missing
                            .iter()
                            .map(|id| format!("“{id}”"))
                            .collect::<Vec<_>>()
                            .join(" or ")
                    )));
                }
                writeln!(out, "{}", catalog.export_json(ids))?;
            }
            Action::ServicesImport(file) => {
                let Some(dir) = dir else {
                    return Err(Fail::Failed(
                        "couldn't locate Playpen's settings folder".to_string(),
                    ));
                };
                let target = dir.join("services").join(ServiceCatalog::stored_name(file));
                let replaced = target.exists();
                let (stored, count) = ServiceCatalog::import_file(dir, file).map_err(failed)?;
                let path = dir.join("services").join(stored);
                let plural = if count == 1 { "" } else { "s" };
                self.done(
                    out,
                    &format!(
                        "{} {count} service{plural} {} {}.",
                        if replaced {
                            "Replaced with"
                        } else {
                            "Imported"
                        },
                        if replaced { "from the file in" } else { "into" },
                        path.display()
                    ),
                    json!({ "imported": count, "file": path, "replaced": replaced }),
                )?;
            }
            Action::Gateways => {
                let gateways = self.openshell()?.gateways().map_err(failed)?;
                if self.json {
                    self.print(out, &gateways_json(&gateways))?;
                } else if gateways.is_empty() {
                    writeln!(err, "OpenShell has no gateways.")?;
                } else {
                    table(
                        out,
                        &["", "NAME", "TYPE", "AUTH", "ENDPOINT"],
                        gateways
                            .iter()
                            .map(|gateway| {
                                vec![
                                    if gateway.active { "*" } else { "" }.to_string(),
                                    gateway.name.clone(),
                                    gateway.kind.clone(),
                                    gateway.auth.clone(),
                                    gateway.endpoint.clone(),
                                ]
                            })
                            .collect(),
                    )?;
                }
            }
            Action::Sandboxes => {
                let sandboxes = self.openshell()?.sandboxes().map_err(failed)?;
                if self.json {
                    self.print(out, &sandboxes_json(&sandboxes))?;
                } else if sandboxes.is_empty() {
                    writeln!(err, "There are no sandboxes on this gateway.")?;
                } else {
                    table(
                        out,
                        &["NAME", "PHASE", "AGENT", "CREATED"],
                        sandboxes
                            .iter()
                            .map(|sandbox| {
                                vec![
                                    sandbox.name.clone(),
                                    phase_name(&sandbox.phase),
                                    agent_of(sandbox),
                                    sandbox.created.clone(),
                                ]
                            })
                            .collect(),
                    )?;
                }
            }
            Action::Providers => {
                let providers = self.openshell()?.providers().map_err(failed)?;
                if self.json {
                    self.print(out, &providers_json(&providers))?;
                } else if providers.is_empty() {
                    writeln!(err, "There are no providers on this gateway.")?;
                } else {
                    table(
                        out,
                        &["NAME", "TYPE"],
                        providers
                            .iter()
                            .map(|provider| vec![provider.name.clone(), provider.kind.clone()])
                            .collect(),
                    )?;
                }
            }
            Action::SandboxStart(name) => {
                self.openshell()?.start(name).map_err(failed)?;
                self.done(
                    out,
                    &format!("Started “{name}”."),
                    json!({ "sandbox": name, "did": "start" }),
                )?;
            }
            Action::SandboxStop(name) => {
                self.openshell()?.stop(name).map_err(failed)?;
                self.done(
                    out,
                    &format!("Stopped “{name}”."),
                    json!({ "sandbox": name, "did": "stop" }),
                )?;
            }
            Action::SandboxRestart(name) => {
                self.openshell()?.restart(name).map_err(failed)?;
                self.done(
                    out,
                    &format!("Restarted “{name}”."),
                    json!({ "sandbox": name, "did": "restart" }),
                )?;
            }
            Action::SandboxDelete { name, yes } => {
                if !yes {
                    return Err(Fail::Usage(format!(
                        "deleting “{name}” can't be undone; add --yes to do it"
                    )));
                }
                self.openshell()?.delete(name).map_err(failed)?;
                self.done(
                    out,
                    &format!("Deleted “{name}”."),
                    json!({ "sandbox": name, "did": "delete" }),
                )?;
            }
            Action::SandboxServices(name) => {
                let shell = self.openshell()?;
                let attached = shell.attached(name).map_err(failed)?;
                let profiles = shell.profiles().map_err(failed)?;
                if self.json {
                    self.print(out, &attached_json(&attached, &profiles))?;
                } else if attached.is_empty() {
                    writeln!(err, "“{name}” has no providers attached.")?;
                } else {
                    table(
                        out,
                        &["PROVIDER", "TYPE", "HOSTS"],
                        attached
                            .iter()
                            .map(|provider| {
                                let hosts = profiles
                                    .iter()
                                    .find(|profile| profile.id == provider.kind)
                                    .map(|profile| {
                                        profile
                                            .endpoints
                                            .iter()
                                            .map(|endpoint| {
                                                format!(
                                                    "{}:{} ({})",
                                                    endpoint.host, endpoint.port, endpoint.access
                                                )
                                            })
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    })
                                    .unwrap_or_default();
                                vec![provider.name.clone(), provider.kind.clone(), hosts]
                            })
                            .collect(),
                    )?;
                }
            }
            Action::SandboxAttach { name, provider } => {
                let shell = self.openshell()?;
                let service = service_for(&shell, provider)?;
                shell.attach_service(name, &service).map_err(failed)?;
                self.done(
                    out,
                    &format!("Attached “{provider}” to “{name}”."),
                    json!({ "sandbox": name, "provider": provider, "did": "attach" }),
                )?;
            }
            Action::SandboxDetach { name, provider } => {
                let shell = self.openshell()?;
                let service = service_for(&shell, provider)?;
                shell.detach_service(name, &service).map_err(failed)?;
                self.done(
                    out,
                    &format!("Detached “{provider}” from “{name}”."),
                    json!({ "sandbox": name, "provider": provider, "did": "detach" }),
                )?;
            }
        }
        Ok(())
    }
}

/// The provider `name` as a service, if it can be one: its profile isn't for a model and sends its key somewhere.
fn service_for(shell: &OpenShell, name: &str) -> Result<Service, Fail> {
    let providers = shell.providers().map_err(failed)?;
    let profiles = shell.profiles().map_err(failed)?;
    let Some(provider) = providers.iter().find(|provider| provider.name == name) else {
        return Err(Fail::Failed(format!(
            "there's no provider “{name}” on this gateway; `playpen providers` lists them"
        )));
    };
    Service::choices(&providers, &profiles)
        .into_iter()
        .find(|(candidate, _)| candidate.name == provider.name)
        .map(|(provider, profile)| Service::new(provider, profile))
        .ok_or_else(|| {
            Fail::Failed(format!(
                "“{name}” can't be a service: it's for a model, or its type sends its key nowhere"
            ))
        })
}

fn phase_name(phase: &Phase) -> String {
    match phase {
        Phase::Provisioning => "provisioning".to_string(),
        Phase::Ready => "ready".to_string(),
        Phase::Stopped => "stopped".to_string(),
        Phase::Suspended => "suspended".to_string(),
        Phase::Completed => "completed".to_string(),
        Phase::Deleting => "deleting".to_string(),
        Phase::Error => "error".to_string(),
        Phase::Other(name) => name.clone(),
    }
}

/// The agent a sandbox runs, as a table shows it: a custom agent by its own name.
fn agent_of(sandbox: &Sandbox) -> String {
    match &sandbox.custom {
        Some(custom) => custom.name.clone(),
        None => sandbox.agent.clone().unwrap_or_default(),
    }
}

fn sandboxes_json(sandboxes: &[Sandbox]) -> Value {
    Value::Array(
        sandboxes
            .iter()
            .map(|sandbox| {
                json!({
                    "name": sandbox.name,
                    "phase": phase_name(&sandbox.phase),
                    "created": sandbox.created,
                    "agent": sandbox.agent,
                    "custom_agent": sandbox.custom.as_ref().map(|custom| &custom.name),
                    "folders": sandbox.folders,
                    "workdir": sandbox.workdir,
                })
            })
            .collect(),
    )
}

fn gateways_json(gateways: &[GatewayInfo]) -> Value {
    Value::Array(
        gateways
            .iter()
            .map(|gateway| {
                json!({
                    "name": gateway.name,
                    "endpoint": gateway.endpoint,
                    "active": gateway.active,
                    "type": gateway.kind,
                    "remote": gateway.is_remote,
                    "auth": gateway.auth,
                })
            })
            .collect(),
    )
}

fn providers_json(providers: &[Provider]) -> Value {
    Value::Array(
        providers
            .iter()
            .map(|provider| json!({ "name": provider.name, "type": provider.kind }))
            .collect(),
    )
}

fn attached_json(attached: &[Provider], profiles: &[playpen_core::Profile]) -> Value {
    Value::Array(
        attached
            .iter()
            .map(|provider| {
                let endpoints: Vec<Value> = profiles
                    .iter()
                    .find(|profile| profile.id == provider.kind)
                    .map(|profile| {
                        profile
                            .endpoints
                            .iter()
                            .map(|endpoint| {
                                json!({
                                    "host": endpoint.host,
                                    "port": endpoint.port,
                                    "protocol": endpoint.protocol,
                                    "access": endpoint.access,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                json!({ "provider": provider.name, "type": provider.kind, "endpoints": endpoints })
            })
            .collect(),
    )
}

/// A catalog entry as JSON, with its host as Playpen reaches it now (Patchbay's port moves).
fn preset_json(service: &ServicePreset) -> Value {
    let mut value = serde_json::to_value(service).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.insert("host".to_string(), Value::String(service.resolved_host()));
    }
    value
}

fn show(out: &mut impl Write, service: &ServicePreset) -> io::Result<()> {
    let value = serde_json::to_value(service).unwrap_or(Value::Null);
    let word = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    writeln!(out, "{} ({})", service.name, service.id)?;
    writeln!(out, "  {}", service.summary)?;
    let mut line = |label: &str, text: String| -> io::Result<()> {
        if text.is_empty() {
            Ok(())
        } else {
            writeln!(out, "  {label:<10} {text}")
        }
    };
    line("category", word("category"))?;
    line("host", service.resolved_host())?;
    line("more hosts", service.extra_hosts.join(", "))?;
    line("variable", service.variable.clone())?;
    line("auth", word("auth"))?;
    line("header", service.header.clone().unwrap_or_default())?;
    line("access", word("access"))?;
    line("keychain", {
        match (&service.keychain_service, &service.keychain_account) {
            (Some(item), Some(account)) => format!("{item} / {account}"),
            (Some(item), None) => item.clone(),
            _ => String::new(),
        }
    })?;
    line("get a key", service.help.clone().unwrap_or_default())?;
    line("note", service.note.clone().unwrap_or_default())
}

/// Prints `rows` under `header`, each column as wide as its widest cell except the last, which is as it comes.
fn table(out: &mut impl Write, header: &[&str], rows: Vec<Vec<String>>) -> io::Result<()> {
    let columns = header.len();
    let mut widths = vec![0; columns];
    for row in std::iter::once(
        header
            .iter()
            .map(|cell| (*cell).to_string())
            .collect::<Vec<_>>(),
    )
    .chain(rows.iter().cloned())
    {
        for (width, cell) in widths.iter_mut().zip(&row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let line = |cells: Vec<String>| {
        let mut line = String::new();
        for (index, cell) in cells.iter().enumerate() {
            if index + 1 == columns {
                line.push_str(cell);
            } else {
                line.push_str(&format!("{cell:<width$}  ", width = widths[index]));
            }
        }
        line.trim_end().to_string()
    };
    writeln!(
        out,
        "{}",
        line(header.iter().map(|cell| (*cell).to_string()).collect())
    )?;
    for row in rows {
        writeln!(out, "{}", line(row))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Action, Command, parse, phase_name, sandboxes_json, table};
    use playpen_core::{Phase, Sandbox};
    use std::path::PathBuf;

    /// Runs the command in an empty settings folder, and gives its status, output and errors.
    fn run(args: &[&str]) -> (i32, String, String) {
        let folder = std::env::temp_dir().join(format!(
            "playpen-cli-test-{}-{}",
            std::process::id(),
            args.join("_").replace(['/', ' '], "")
        ));
        run_in(&folder, args)
    }

    fn run_in(folder: &std::path::Path, args: &[&str]) -> (i32, String, String) {
        std::fs::create_dir_all(folder).unwrap();
        let command = parse(args).expect("a command");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let status = command
            .run_in(Some(folder.to_path_buf()), &mut out, &mut err)
            .unwrap();
        (
            status,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn wrong(args: &[&str]) -> String {
        match parse(args) {
            Some(Command {
                action: Action::Wrong(reason),
                ..
            }) => reason,
            other => panic!("expected a mistake, got {other:?}"),
        }
    }

    #[test]
    fn version_prints_the_version_and_build_and_as_json() {
        let (status, out, _) = run(&["--version"]);
        assert_eq!(
            (status, out.trim()),
            (0, format!("playpen {}", playpen_core::version()).as_str())
        );
        let (_, out, _) = run(&["version", "--json"]);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["version"], playpen_core::VERSION);
    }

    #[test]
    fn help_says_how_to_use_it_wherever_it_is_asked_for() {
        for args in [
            &["-h"][..],
            &["help"],
            &["services", "--help"],
            &["--help", "sandbox", "start"],
        ] {
            let (status, out, _) = run(args);
            assert_eq!(status, 0, "{args:?}");
            assert!(out.starts_with("usage: playpen"), "{args:?}");
        }
    }

    #[test]
    fn services_list_search_show_and_export_the_catalog() {
        let (status, out, err) = run(&["services", "statsig"]);
        assert_eq!((status, err.as_str()), (0, ""));
        assert!(out.starts_with("ID"), "a header: {out}");
        assert!(out.contains("statsigapi.net"), "{out}");

        let (_, out, _) = run(&["services", "show", "statsig"]);
        assert!(
            out.contains("STATSIG-API-KEY") || out.contains("statsigapi.net"),
            "{out}"
        );

        let (_, out, _) = run(&["services", "list", "statsig", "--json"]);
        let listed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(
            listed
                .as_array()
                .unwrap()
                .iter()
                .any(|service| service["id"] == "statsig")
        );

        let (_, out, _) = run(&["services", "export", "patchbay"]);
        let parsed = playpen_core::ServiceCatalogFile::from_json(&out).unwrap();
        assert_eq!(parsed.services.len(), 1);
        assert_eq!(parsed.services[0].id, "patchbay");
    }

    #[test]
    fn a_search_with_no_match_says_so_on_stderr_and_prints_nothing_else() {
        let (status, out, err) = run(&["services", "nomatchxyz"]);
        assert_eq!(status, 0);
        assert_eq!(out, "");
        assert!(err.contains("No services match"), "{err}");
        let (_, out, _) = run(&["services", "nomatchxyz", "--json"]);
        assert_eq!(out.trim(), "[]");
    }

    #[test]
    fn exporting_or_showing_a_service_that_isnt_there_fails_and_names_it() {
        for args in [
            &["services", "export", "patchbay", "nope"][..],
            &["services", "show", "nope"],
        ] {
            let (status, out, err) = run(args);
            assert_eq!(status, 1, "{args:?}");
            assert_eq!(out, "", "nothing to import is printed: {args:?}");
            assert!(err.contains("“nope”"), "{err}");
        }
    }

    #[test]
    fn importing_keeps_a_catalog_and_says_when_it_replaces_one() {
        let folder =
            std::env::temp_dir().join(format!("playpen-cli-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let (_, shared, _) = run_in(&folder, &["services", "export", "patchbay", "github"]);
        let file: PathBuf = folder.join("My Shared!.json");
        std::fs::write(&file, shared).unwrap();
        let file = file.to_str().unwrap();

        let (status, out, _) = run_in(&folder, &["services", "import", file]);
        assert_eq!(status, 0);
        assert!(out.starts_with("Imported 2 services"), "{out}");
        assert!(folder.join("services/my-shared.json").is_file());

        let (_, out, _) = run_in(&folder, &["services", "import", file, "--json"]);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["replaced"], true);
        assert_eq!(parsed["imported"], 2);
    }

    #[test]
    fn importing_a_file_that_cant_be_read_or_isnt_a_catalog_fails() {
        let folder = std::env::temp_dir().join(format!("playpen-cli-bad-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let (status, _, err) = run_in(&folder, &["services", "import", "/nonexistent/file.json"]);
        assert_eq!(status, 1);
        assert!(
            err.contains("Couldn't read") && !err.contains("save"),
            "{err}"
        );
        let junk = folder.join("junk.json");
        std::fs::write(&junk, "not json").unwrap();
        let (status, _, err) = run_in(&folder, &["services", "import", junk.to_str().unwrap()]);
        assert_eq!(status, 1);
        assert!(err.contains("isn't valid"), "{err}");
    }

    #[test]
    fn a_catalog_file_that_is_broken_warns_on_stderr_and_the_list_still_prints() {
        let folder = std::env::temp_dir().join(format!("playpen-cli-warn-{}", std::process::id()));
        std::fs::create_dir_all(folder.join("services")).unwrap();
        std::fs::write(folder.join("services/zz.json"), "junk").unwrap();
        let (status, out, err) = run_in(&folder, &["services", "statsig", "--json"]);
        assert_eq!(status, 0);
        assert!(
            err.starts_with("warning:") && err.contains("zz.json"),
            "{err}"
        );
        serde_json::from_str::<serde_json::Value>(&out).expect("stdout is still only JSON");
    }

    #[test]
    fn a_mistake_in_the_command_line_exits_2_with_the_usage_on_stderr() {
        let (status, out, err) = run(&["services", "import"]);
        assert_eq!((status, out.as_str()), (2, ""));
        assert!(
            err.contains("give the path") && err.contains("usage: playpen"),
            "{err}"
        );
    }

    #[test]
    fn options_a_command_doesnt_take_are_refused_by_name() {
        assert_eq!(wrong(&["services", "-x"]), "services doesn't take -x");
        assert_eq!(
            wrong(&["services", "list", "-x"]),
            "services doesn't take -x"
        );
        assert_eq!(
            wrong(&["services", "export", "--bogus"]),
            "services doesn't take --bogus"
        );
        assert_eq!(
            wrong(&["version", "--bogus"]),
            "version doesn't take --bogus"
        );
        assert_eq!(wrong(&["version", "extra"]), "version takes no arguments");
        assert_eq!(wrong(&["sandboxes", "x"]), "sandboxes takes no arguments");
        assert_eq!(
            wrong(&["sandbox", "start", "a", "--yes"]),
            "sandbox doesn't take --yes"
        );
        assert_eq!(wrong(&["--json"]), "--json needs a command");
        assert_eq!(
            wrong(&["--gateway"]),
            "--gateway needs the name of a gateway"
        );
    }

    #[test]
    fn a_misspelt_command_or_option_is_an_error_and_not_a_window() {
        assert_eq!(
            wrong(&["servces"]),
            "unknown command “servces”; did you mean “services”?"
        );
        assert_eq!(
            wrong(&["sandbxes"]),
            "unknown command “sandbxes”; did you mean “sandboxes”?"
        );
        assert_eq!(wrong(&["--verison"]), "unknown option --verison");
        assert_eq!(wrong(&["--bogus", "services"]), "unknown option --bogus");
    }

    #[test]
    fn what_macos_and_the_finder_pass_opens_the_app() {
        assert_eq!(parse::<&str>(&[]), None);
        assert_eq!(parse(&["-psn_0_12345"]), None);
        assert_eq!(parse(&["-NSDocumentRevisionsDebugMode", "YES"]), None);
        assert_eq!(parse(&["notes.txt"]), None);
        assert_eq!(parse(&["/Users/me/folder"]), None);
        assert_eq!(parse(&["hello"]), None);
    }

    #[test]
    fn sandbox_commands_take_their_names_and_a_delete_needs_yes() {
        let action = |args: &[&str]| parse(args).unwrap().action;
        assert_eq!(action(&["sandboxes"]), Action::Sandboxes);
        assert_eq!(action(&["sandbox"]), Action::Sandboxes);
        assert_eq!(
            action(&["sandbox", "start", "a"]),
            Action::SandboxStart("a".into())
        );
        assert_eq!(
            action(&["sandbox", "attach", "a", "github"]),
            Action::SandboxAttach {
                name: "a".into(),
                provider: "github".into()
            }
        );
        assert_eq!(
            action(&["sandbox", "delete", "a", "--yes"]),
            Action::SandboxDelete {
                name: "a".into(),
                yes: true
            }
        );
        let (status, _, err) = run(&["sandbox", "delete", "a"]);
        assert_eq!(status, 2, "it didn't delete: {err}");
        assert!(err.contains("add --yes"), "{err}");
        assert!(wrong(&["sandbox", "start"]).contains("name of a sandbox"));
        assert!(wrong(&["sandbox", "attach", "a"]).contains("a sandbox and a provider"));
        assert!(wrong(&["sandbox", "nope"]).contains("“nope”"));
    }

    #[test]
    fn the_gateway_is_taken_from_either_form_and_from_before_or_after() {
        let gateway = |args: &[&str]| parse(args).unwrap().gateway;
        assert_eq!(
            gateway(&["sandboxes", "--gateway", "kind"]),
            Some("kind".to_string())
        );
        assert_eq!(
            gateway(&["--gateway=kind", "sandboxes"]),
            Some("kind".to_string())
        );
        assert_eq!(gateway(&["sandboxes"]), None);
    }

    #[test]
    fn sandboxes_are_listed_as_json_with_what_playpen_knows_of_them() {
        let sandbox = Sandbox {
            name: "work".into(),
            phase: Phase::Other("odd".into()),
            created: "2026-11-20 03:08:15".into(),
            agent: Some("claude".into()),
            folders: vec!["/a".into()],
            workdir: None,
            custom: None,
        };
        let listed = sandboxes_json(&[sandbox]);
        assert_eq!(listed[0]["phase"], "odd");
        assert_eq!(listed[0]["agent"], "claude");
        assert!(listed[0]["workdir"].is_null());
        assert_eq!(phase_name(&Phase::Ready), "ready");
    }

    #[test]
    fn a_table_lines_its_columns_up_under_a_header_and_leaves_the_last_alone() {
        let mut out = Vec::new();
        table(
            &mut out,
            &["NAME", "NOTE"],
            vec![
                vec!["a".into(), "one".into()],
                vec!["longer".into(), "two".into()],
            ],
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "NAME    NOTE\na       one\nlonger  two\n"
        );
    }
}
