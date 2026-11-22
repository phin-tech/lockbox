//! A gateway on this Mac whose sandboxes are microVMs, which Playpen sets up and runs beside the one OpenShell's
//! Homebrew service runs with Docker: a gateway has one compute driver, so microVMs need a gateway of their own.
//!
//! It runs as a launch agent, so its sandboxes outlive Playpen, with mTLS like the Homebrew gateway, from certificates
//! of its own. When it restarts, its sandboxes start again with their disks, losing only what they hadn't written to
//! them yet, as after a power cut. Everything it keeps is in one folder, short enough for the driver's socket
//! to fit macOS's limit on a socket's path: its certificates, its database, and its sandboxes' disks and image cache.
//! Removing the gateway stops it and keeps that folder, since the disks hold what the agents did.

use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{GATEWAY_VARIABLE, OpenShell, said};
use crate::docker::Docker;
use crate::files::write_atomically;

/// The name OpenShell and Playpen know the gateway by.
pub const MICROVM_GATEWAY: &str = "microvm";

/// The launch agent's label.
const LABEL: &str = "tech.phin.playpen.microvm";

/// The first port tried for the gateway; the Homebrew gateway has 17670.
const FIRST_PORT: u16 = 17671;

/// The image every microVM boots from before it unpacks a sandbox's own: the one OpenShell's own VM gateway uses.
const BOOTSTRAP_IMAGE: &str = "nvcr.io/nvidia/base/ubuntu:24.04";

/// How long the gateway has to start listening once launchd starts it.
const STARTING: Duration = Duration::from_secs(30);

/// How big each microVM is. The driver gives every sandbox the same, so it's the gateway's to choose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MicroVmSize {
    pub cpus: u32,
    pub memory_gib: u32,
    pub disk_gib: u32,
}

impl Default for MicroVmSize {
    /// Room for an agent, a repository and its build: OpenShell's own 2 CPUs, 2 GB and 4 GB of disk are tight for an
    /// agent's image alone.
    fn default() -> Self {
        MicroVmSize {
            cpus: 4,
            memory_gib: 4,
            disk_gib: 16,
        }
    }
}

impl MicroVmSize {
    pub const CPUS: [u32; 3] = [2, 4, 8];
    pub const MEMORY_GIB: [u32; 3] = [2, 4, 8];
    pub const DISK_GIB: [u32; 3] = [8, 16, 32];
}

/// Something the gateway needs that this Mac hasn't got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Missing {
    /// OpenShell's gateway program, or its VM driver, which Homebrew's `openshell` puts beside it.
    Driver,
    /// e2fsprogs, whose `mke2fs` and `debugfs` make the microVMs' disks.
    E2fsprogs,
    /// Docker, which the driver takes Playpen's images from.
    Docker,
}

/// What setting it up is doing, for the person to see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetUpStep {
    Certificates,
    Starting,
    Registering,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MicroVmError {
    #[error("This Mac hasn't everything a MicroVM gateway needs.")]
    Missing,
    #[error("Couldn't {0}: {1}")]
    Failed(String, String),
    #[error("The MicroVM gateway didn't start listening on port {0}. What it said is in {1}.")]
    NotStarted(u16, String),
    #[error(
        "Homebrew isn't installed, so Playpen can't install e2fsprogs. Install it from brew.sh."
    )]
    NoBrew,
}

fn failed(doing: &str, error: impl ToString) -> MicroVmError {
    MicroVmError::Failed(doing.to_string(), error.to_string())
}

/// Where the gateway's files go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicroVmPlaces {
    /// Its certificates, configuration, database and sandboxes' disks.
    pub root: PathBuf,
    pub launch_agent: PathBuf,
    pub log: PathBuf,
}

impl MicroVmPlaces {
    /// The places under the home folder `home`.
    pub fn in_home(home: &Path) -> MicroVmPlaces {
        MicroVmPlaces {
            root: home.join(".local/state/playpen/microvm"),
            launch_agent: home.join(format!("Library/LaunchAgents/{LABEL}.plist")),
            log: home.join("Library/Logs/tech.phin.playpen/microvm-gateway.log"),
        }
    }

    fn config(&self) -> PathBuf {
        self.root.join("gateway.toml")
    }

    fn tls(&self) -> PathBuf {
        self.root.join("tls")
    }

    fn database(&self) -> PathBuf {
        self.root.join("gateway.db")
    }

    fn state(&self) -> PathBuf {
        self.root.join("state")
    }

    /// Where `generate-certs` puts the copy of the client's certificates it makes for OpenShell's command line.
    fn generating_config(&self) -> PathBuf {
        self.root.join("generating")
    }
}

/// OpenShell's programs for running a gateway: the gateway, and the folder its VM driver is in.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Programs {
    gateway: PathBuf,
    driver_dir: PathBuf,
}

impl Programs {
    /// Homebrew's, through its `opt` link, which stays put when OpenShell is upgraded, or else beside `openshell`
    /// wherever it really is.
    fn beside(openshell: &Path) -> Option<Programs> {
        let mut places = Vec::new();
        if let Some(prefix) = openshell.parent().and_then(Path::parent) {
            places.push(prefix.join("opt/openshell"));
        }
        if let Some(installed) = std::fs::canonicalize(openshell)
            .ok()
            .and_then(|real| real.parent().and_then(Path::parent).map(Path::to_path_buf))
        {
            places.push(installed);
        }
        places.into_iter().find_map(|place| {
            let programs = Programs {
                gateway: place.join("bin/openshell-gateway"),
                driver_dir: place.join("libexec"),
            };
            (programs.gateway.is_file()
                && programs.driver_dir.join("openshell-driver-vm").is_file())
            .then_some(programs)
        })
    }
}

/// Where the driver looks for e2fsprogs besides `PATH`, which a launch agent hasn't got.
const E2FSPROGS: [&str; 4] = [
    "/opt/homebrew/opt/e2fsprogs/sbin",
    "/usr/local/opt/e2fsprogs/sbin",
    "/opt/homebrew/sbin",
    "/usr/local/sbin",
];

fn has_e2fsprogs() -> bool {
    E2FSPROGS.iter().any(|dir| {
        Path::new(dir).join("debugfs").is_file() && Path::new(dir).join("mke2fs").is_file()
    })
}

/// The gateway's configuration: the VM driver, with its state under `places`, reaching the gateway at `port` with the
/// gateway's own CA, and each microVM `size`.
fn gateway_toml(
    places: &MicroVmPlaces,
    programs: &Programs,
    port: u16,
    size: MicroVmSize,
) -> String {
    let quoted = |path: &Path| toml::Value::String(path.display().to_string()).to_string();
    format!(
        "# Written by Playpen: a gateway whose sandboxes are microVMs.\n\
         [openshell]\n\
         version = 2\n\
         \n\
         [openshell.gateway]\n\
         compute_driver = \"vm\"\n\
         guest_tls_ca = {ca}\n\
         \n\
         [openshell.drivers.vm]\n\
         default_image = \"{BOOTSTRAP_IMAGE}\"\n\
         grpc_endpoint = \"https://127.0.0.1:{port}\"\n\
         driver_dir = {driver_dir}\n\
         state_dir = {state}\n\
         vcpus = {cpus}\n\
         mem_mib = {memory}\n\
         overlay_disk_mib = {disk}\n",
        ca = quoted(&places.tls().join("ca.crt")),
        driver_dir = quoted(&programs.driver_dir),
        state = quoted(&places.state()),
        cpus = size.cpus,
        memory = size.memory_gib * 1024,
        disk = size.disk_gib * 1024,
    )
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The launch agent that runs the gateway at `port` and starts it again if it stops, with only what it needs in its
/// environment: its certificates, and Docker, which launchd's bare environment wouldn't find otherwise.
fn launch_agent(
    places: &MicroVmPlaces,
    programs: &Programs,
    home: &Path,
    port: u16,
    docker_host: Option<&str>,
) -> String {
    let string = |text: &str| format!("\t\t<string>{}</string>\n", xml(text));
    let mut arguments = String::new();
    for argument in [
        programs.gateway.display().to_string(),
        "--config".to_string(),
        places.config().display().to_string(),
        "--name".to_string(),
        MICROVM_GATEWAY.to_string(),
        "--db-url".to_string(),
        format!("sqlite:{}?mode=rwc", places.database().display()),
        "--port".to_string(),
        port.to_string(),
    ] {
        arguments.push_str(&string(&argument));
    }
    let mut environment = vec![
        ("HOME", home.display().to_string()),
        ("PATH", "/usr/bin:/bin:/usr/sbin:/sbin".to_string()),
        (
            "OPENSHELL_LOCAL_TLS_DIR",
            places.tls().display().to_string(),
        ),
    ];
    if let Some(host) = docker_host {
        environment.push(("DOCKER_HOST", host.to_string()));
    }
    let environment: String = environment
        .iter()
        .map(|(name, value)| {
            format!(
                "\t\t<key>{name}</key>\n\t\t<string>{}</string>\n",
                xml(value)
            )
        })
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{LABEL}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n\
         {arguments}\
         \t</array>\n\
         \t<key>EnvironmentVariables</key>\n\
         \t<dict>\n\
         {environment}\
         \t</dict>\n\
         \t<key>WorkingDirectory</key>\n\
         \t<string>{root}</string>\n\
         \t<key>RunAtLoad</key>\n\
         \t<true/>\n\
         \t<key>KeepAlive</key>\n\
         \t<true/>\n\
         \t<key>StandardOutPath</key>\n\
         \t<string>{log}</string>\n\
         \t<key>StandardErrorPath</key>\n\
         \t<string>{log}</string>\n\
         </dict>\n\
         </plist>\n",
        root = xml(&places.root.display().to_string()),
        log = xml(&places.log.display().to_string()),
    )
}

/// The command that makes the gateway's certificates in its own folder. `generate-certs` also copies the client's into
/// OpenShell's folder for the gateway called `openshell`, which is the Homebrew gateway's, and would lock its clients
/// out of it; so it's given a configuration folder of its own to copy into, which is deleted after.
fn generate_command(programs: &Programs, places: &MicroVmPlaces) -> Command {
    let mut command = Command::new(&programs.gateway);
    command
        .args(["generate-certs", "--output-dir"])
        .arg(places.tls())
        .args(["--server-san", "localhost", "--server-san", "127.0.0.1"])
        .env("XDG_CONFIG_HOME", places.generating_config())
        .env("NO_COLOR", "1")
        .stdin(Stdio::null());
    command
}

/// The first port from [`FIRST_PORT`] nothing's listening on.
fn free_port() -> Option<u16> {
    (FIRST_PORT..FIRST_PORT + 100)
        .find(|port| TcpListener::bind((Ipv4Addr::LOCALHOST, *port)).is_ok())
}

/// The port the configuration at `config` gives the gateway, if it's there.
fn configured_port(config: &Path) -> Option<u16> {
    let text = std::fs::read_to_string(config).ok()?;
    let value: toml::Table = text.parse().ok()?;
    let endpoint = value
        .get("openshell")?
        .get("drivers")?
        .get("vm")?
        .get("grpc_endpoint")?
        .as_str()?;
    endpoint.rsplit(':').next()?.parse().ok()
}

/// The person's user ID, for launchd's domain of their agents.
fn uid() -> Result<String, MicroVmError> {
    let output = Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .map_err(|error| failed("find your user ID", error))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn launchctl(doing: &str, args: &[&str]) -> Result<(), MicroVmError> {
    let output = Command::new("/bin/launchctl")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| failed(doing, error))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(failed(doing, said(&output.stderr)))
    }
}

/// Whether launchd still has the gateway's service, running or going away.
fn has_service(uid: &str) -> bool {
    Command::new("/bin/launchctl")
        .args(["print", &format!("gui/{uid}/{LABEL}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Stops the gateway's service, if launchd has it, and waits until launchd has let it go: until then, starting it
/// again fails.
fn stop_service(uid: &str) {
    if !has_service(uid) {
        return;
    }
    let _ = launchctl(
        "stop the MicroVM gateway",
        &["bootout", &format!("gui/{uid}/{LABEL}")],
    );
    let started = Instant::now();
    while has_service(uid) && started.elapsed() < STARTING {
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Starts the service the launch agent at `agent` describes. launchd can refuse for a moment after one was stopped,
/// so it's tried a few times.
fn start_service(uid: &str, agent: &Path) -> Result<(), MicroVmError> {
    let domain = format!("gui/{uid}");
    let agent = agent.display().to_string();
    let mut tries = 0;
    loop {
        match launchctl("start the MicroVM gateway", &["bootstrap", &domain, &agent]) {
            Err(_) if tries < 5 => {
                tries += 1;
                std::thread::sleep(Duration::from_secs(1));
            }
            done => return done,
        }
    }
}

/// The last few lines of the gateway's log, to say why it didn't start.
fn log_tail(log: &Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines[lines.len().saturating_sub(5)..].join("\n")
}

/// The MicroVM gateway on this Mac: whether it can be set up, setting it up, and removing it.
#[derive(Debug, Clone)]
pub struct MicroVm {
    openshell: OpenShell,
    home: PathBuf,
    places: MicroVmPlaces,
    programs: Option<Programs>,
    docker: Option<Docker>,
}

impl MicroVm {
    /// The gateway `openshell` would run, with its files in the home folder.
    pub fn find(openshell: &OpenShell) -> Option<MicroVm> {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        Some(MicroVm {
            openshell: openshell.without_gateway(),
            places: MicroVmPlaces::in_home(&home),
            home,
            programs: Programs::beside(openshell.binary()),
            docker: Docker::find().ok(),
        })
    }

    pub fn places(&self) -> &MicroVmPlaces {
        &self.places
    }

    /// What this Mac hasn't got that the gateway needs.
    pub fn missing(&self) -> Vec<Missing> {
        let mut missing = Vec::new();
        if self.programs.is_none() {
            missing.push(Missing::Driver);
        }
        if !has_e2fsprogs() {
            missing.push(Missing::E2fsprogs);
        }
        if self.docker.is_none() {
            missing.push(Missing::Docker);
        }
        missing
    }

    /// The size it was set up with, read back from its configuration, if it's been set up.
    pub fn configured_size(&self) -> Option<MicroVmSize> {
        let text = std::fs::read_to_string(self.places.config()).ok()?;
        let table: toml::Table = text.parse().ok()?;
        let vm = table.get("openshell")?.get("drivers")?.get("vm")?;
        let number = |key: &str| {
            vm.get(key)?
                .as_integer()
                .and_then(|value| u32::try_from(value).ok())
        };
        Some(MicroVmSize {
            cpus: number("vcpus")?,
            memory_gib: number("mem_mib")? / 1024,
            disk_gib: number("overlay_disk_mib")? / 1024,
        })
    }

    /// Whether Playpen has set it up.
    pub fn is_set_up(&self) -> bool {
        self.places.launch_agent.is_file()
    }

    /// Sets the gateway up, or sets it up again with `size`, which restarts it and stops its sandboxes. `step` hears
    /// what it's doing.
    pub fn set_up(&self, size: MicroVmSize, step: impl Fn(SetUpStep)) -> Result<(), MicroVmError> {
        let Some(programs) = &self.programs else {
            return Err(MicroVmError::Missing);
        };
        if !self.missing().is_empty() {
            return Err(MicroVmError::Missing);
        }
        step(SetUpStep::Certificates);
        self.make_folders()?;
        if !self.places.tls().join("client/tls.key").is_file() {
            self.generate_certificates(programs)?;
        }
        step(SetUpStep::Starting);
        let uid = uid()?;
        // A gateway set up before stops first, so its port is free to take again.
        stop_service(&uid);
        let port = configured_port(&self.places.config())
            .filter(|port| TcpListener::bind((Ipv4Addr::LOCALHOST, *port)).is_ok())
            .or_else(free_port)
            .ok_or_else(|| {
                failed(
                    "find a free port for the MicroVM gateway",
                    "every one tried is taken",
                )
            })?;
        let docker_host = self.docker.as_ref().and_then(Docker::host);
        write_atomically(
            &self.places.config(),
            gateway_toml(&self.places, programs, port, size).as_bytes(),
        )
        .map_err(|error| failed("write the MicroVM gateway's configuration", error))?;
        write_atomically(
            &self.places.launch_agent,
            launch_agent(
                &self.places,
                programs,
                &self.home,
                port,
                docker_host.as_deref(),
            )
            .as_bytes(),
        )
        .map_err(|error| failed("write the MicroVM gateway's launch agent", error))?;
        start_service(&uid, &self.places.launch_agent)?;
        self.wait_for(port)?;
        step(SetUpStep::Registering);
        self.register(port)
    }

    fn make_folders(&self) -> Result<(), MicroVmError> {
        use std::os::unix::fs::PermissionsExt as _;
        for folder in [
            Some(self.places.root.as_path()),
            self.places.launch_agent.parent(),
            self.places.log.parent(),
        ]
        .into_iter()
        .flatten()
        {
            std::fs::create_dir_all(folder)
                .map_err(|error| failed(&format!("make {}", folder.display()), error))?;
        }
        // The certificates and the database are the person's alone.
        std::fs::set_permissions(&self.places.root, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| failed("keep the MicroVM gateway's folder private", error))
    }

    fn generate_certificates(&self, programs: &Programs) -> Result<(), MicroVmError> {
        let doing = "make the MicroVM gateway's certificates";
        let output = generate_command(programs, &self.places)
            .output()
            .map_err(|error| failed(doing, error));
        let _ = std::fs::remove_dir_all(self.places.generating_config());
        let output = output?;
        if output.status.success() {
            Ok(())
        } else {
            Err(failed(doing, said(&output.stderr)))
        }
    }

    fn wait_for(&self, port: u16) -> Result<(), MicroVmError> {
        let started = Instant::now();
        while started.elapsed() < STARTING {
            if TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_ok() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err(MicroVmError::NotStarted(
            port,
            self.places.log.display().to_string(),
        ))
    }

    /// Registers the gateway at `port` with OpenShell, with its own certificates rather than the Homebrew gateway's,
    /// which `gateway add --local` takes when it isn't told otherwise, and leaves the active gateway as it was.
    fn register(&self, port: u16) -> Result<(), MicroVmError> {
        let doing = "register the MicroVM gateway with OpenShell";
        let active = self
            .openshell
            .gateways()
            .ok()
            .and_then(|all| all.into_iter().find(|gateway| gateway.active))
            .map(|gateway| gateway.name);
        // One registered before may have had another port.
        let _ = self
            .openshell
            .run(doing, &["gateway", "remove", MICROVM_GATEWAY]);
        let endpoint = format!("https://127.0.0.1:{port}");
        let output = self
            .openshell
            .command(
                &[
                    "gateway",
                    "add",
                    &endpoint,
                    "--local",
                    "--name",
                    MICROVM_GATEWAY,
                ],
                None,
            )
            .env_remove(GATEWAY_VARIABLE)
            .env("OPENSHELL_LOCAL_TLS_DIR", self.places.tls())
            .stdin(Stdio::null())
            .output()
            .map_err(|error| failed(doing, error))?;
        if let Some(name) = active.filter(|name| name != MICROVM_GATEWAY) {
            let _ = self.openshell.run(
                "go back to the active gateway",
                &["gateway", "select", &name],
            );
        }
        if output.status.success() {
            Ok(())
        } else {
            Err(failed(doing, said(&output.stderr)))
        }
    }

    /// Stops the gateway, which stops its sandboxes, and has OpenShell forget it. The folder with its certificates and
    /// its sandboxes' disks stays, and is given back.
    pub fn remove(&self) -> Result<PathBuf, MicroVmError> {
        let uid = uid()?;
        stop_service(&uid);
        if self.places.launch_agent.exists() {
            std::fs::remove_file(&self.places.launch_agent)
                .map_err(|error| failed("delete the MicroVM gateway's launch agent", error))?;
        }
        // Not registered is as good as forgotten.
        let _ = self.openshell.run(
            "forget the MicroVM gateway",
            &["gateway", "remove", MICROVM_GATEWAY],
        );
        // OpenShell's copy of the client certificate Playpen made, which nothing else uses.
        if let Some(folder) = super::register::config_home()
            .map(|config| config.join("openshell/gateways").join(MICROVM_GATEWAY))
            .filter(|folder| folder.exists())
        {
            std::fs::remove_dir_all(&folder).map_err(|error| {
                failed(
                    "delete OpenShell's certificates for the MicroVM gateway",
                    error,
                )
            })?;
        }
        Ok(self.places.root.clone())
    }
}

/// Installs e2fsprogs with Homebrew.
pub fn install_e2fsprogs() -> Result<(), MicroVmError> {
    let brew = ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]
        .into_iter()
        .map(PathBuf::from)
        .find(|brew| brew.is_file())
        .ok_or(MicroVmError::NoBrew)?;
    let output = Command::new(brew)
        .args(["install", "e2fsprogs"])
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .env("NONINTERACTIVE", "1")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| failed("install e2fsprogs", error))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(failed("install e2fsprogs", said(&output.stderr)))
    }
}

/// Why the gateway didn't start, from its log, for when [`MicroVmError::NotStarted`] is shown.
pub fn why_not_started(places: &MicroVmPlaces) -> String {
    log_tail(&places.log)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        MicroVmPlaces, MicroVmSize, Programs, configured_port, gateway_toml, generate_command,
        launch_agent,
    };

    fn programs() -> Programs {
        Programs {
            gateway: "/opt/homebrew/opt/openshell/bin/openshell-gateway".into(),
            driver_dir: "/opt/homebrew/opt/openshell/libexec".into(),
        }
    }

    #[test]
    fn the_microvm_gateways_configuration_names_its_driver_size_and_ca() {
        let places = MicroVmPlaces::in_home(Path::new("/Users/someone"));
        insta::assert_snapshot!(gateway_toml(
            &places,
            &programs(),
            17671,
            MicroVmSize::default()
        ));
    }

    #[test]
    fn the_launch_agent_runs_the_gateway_with_its_own_certificates_and_docker() {
        let places = MicroVmPlaces::in_home(Path::new("/Users/someone"));
        insta::assert_snapshot!(launch_agent(
            &places,
            &programs(),
            Path::new("/Users/someone"),
            17671,
            Some("unix:///Users/someone/.orbstack/run/docker.sock"),
        ));
    }

    #[test]
    fn the_port_set_up_before_is_read_back_from_the_configuration() {
        let dir = std::env::temp_dir().join(format!("microvm-port-{}", std::process::id()));
        let places = MicroVmPlaces {
            root: dir.clone(),
            launch_agent: dir.join("agent.plist"),
            log: dir.join("log"),
        };
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            places.root.join("gateway.toml"),
            gateway_toml(&places, &programs(), 17673, MicroVmSize::default()),
        )
        .unwrap();
        assert_eq!(
            configured_port(&places.root.join("gateway.toml")),
            Some(17673)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_drivers_socket_fits_macos_limit_for_a_long_user_name() {
        let places = MicroVmPlaces::in_home(Path::new("/Users/a-rather-long-user-name"));
        let socket = places.root.join("state/run/compute-driver.sock");
        assert!(socket.as_os_str().len() < 104, "{}", socket.display());
    }

    #[test]
    fn making_certificates_never_touches_the_openshell_gateways_own() {
        let places = MicroVmPlaces::in_home(Path::new("/Users/someone"));
        let command = generate_command(&programs(), &places);
        let config = command
            .get_envs()
            .find(|(name, _)| *name == "XDG_CONFIG_HOME")
            .and_then(|(_, value)| value)
            .map(Path::new);
        assert_eq!(config, Some(places.root.join("generating").as_path()));
    }
}
