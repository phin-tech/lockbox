//! Ports forwarded from this Mac to a sandbox, so what's served inside, such as a dev server, opens in the Mac's
//! browser. OpenShell keeps each forward running in the background on the same port on this Mac's loopback, and lists
//! them with no word of which gateway they're for, so Playpen keeps its own list of which is whose.

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{OpenShell, OpenShellError, said};
use crate::files::write_atomically;

/// A port forwarded to a sandbox: the same number on this Mac and inside it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Forward {
    /// The gateway the sandbox is on, by name, or empty for the active one.
    pub gateway: String,
    pub sandbox: String,
    pub port: u16,
}

impl Forward {
    /// Where it's opened on this Mac.
    pub fn address(&self) -> String {
        format!("http://localhost:{}", self.port)
    }

    /// Whether something answers on its port on this Mac.
    pub fn answers(&self) -> bool {
        TcpStream::connect_timeout(
            &SocketAddr::from((Ipv4Addr::LOCALHOST, self.port)),
            Duration::from_millis(300),
        )
        .is_ok()
    }
}

/// The forwards Playpen started, kept in a file so they're known again after it restarts.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Forwards {
    #[serde(default)]
    pub forwards: Vec<Forward>,
}

impl Forwards {
    pub fn load(path: &Path) -> Forwards {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
        }
        write_atomically(path, text.as_bytes()).map_err(|error| error.to_string())
    }

    /// Those of the sandbox `sandbox` on the gateway `gateway`.
    pub fn of<'a>(
        &'a self,
        gateway: &'a str,
        sandbox: &'a str,
    ) -> impl Iterator<Item = &'a Forward> {
        self.forwards
            .iter()
            .filter(move |forward| forward.gateway == gateway && forward.sandbox == sandbox)
    }

    /// Keeps only those OpenShell still has running, as it listed them.
    pub fn keep_running(&mut self, running: &[ListedForward]) {
        self.forwards.retain(|forward| {
            running
                .iter()
                .any(|listed| listed.port == forward.port && listed.sandbox == forward.sandbox)
        });
    }
}

/// A forward as `openshell forward list` gives it. It says nothing of the gateway, and its `alive` isn't to be
/// trusted, so neither is read.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ListedForward {
    pub sandbox: String,
    pub port: u16,
}

/// Why a port couldn't be forwarded.
fn port_taken(port: u16) -> bool {
    std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_err()
}

impl OpenShell {
    /// The forwards OpenShell has running, on every gateway.
    pub fn forwards(&self) -> Result<Vec<ListedForward>, OpenShellError> {
        let json = self.run(
            "list the forwarded ports",
            &["forward", "list", "-o", "json"],
        )?;
        serde_json::from_str(&json).map_err(|error| OpenShellError::Unreadable(error.to_string()))
    }

    /// Forwards `port` on this Mac's loopback to the same port in the sandbox `name`, in the background. Its output
    /// goes nowhere: the forward it leaves running would otherwise hold a pipe open, and this would never return.
    pub fn forward_port(&self, name: &str, port: u16) -> Result<(), OpenShellError> {
        let doing = format!("forward port {port} to “{name}”");
        if port_taken(port) {
            return Err(OpenShellError::Failed(
                doing,
                format!("something on this Mac is using port {port} already"),
            ));
        }
        let port_text = port.to_string();
        // What it says goes to a file, which the forward left running can hold open without anyone waiting on it.
        let said_to =
            std::env::temp_dir().join(format!("playpen-forward-{}-{port}.log", std::process::id()));
        let file = std::fs::File::create(&said_to)
            .map_err(|error| OpenShellError::Failed(doing.clone(), error.to_string()))?;
        let status = self
            .command(
                &["forward", "start", &port_text, name, "--background"],
                None,
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(file)
            .status()
            .map_err(|error| OpenShellError::Run(error.to_string()));
        let printed = std::fs::read(&said_to).unwrap_or_default();
        let _ = std::fs::remove_file(&said_to);
        if status?.success() {
            Ok(())
        } else {
            Err(OpenShellError::Failed(doing, said(&printed)))
        }
    }

    /// Stops forwarding `port` to the sandbox `name`.
    pub fn stop_forward(&self, name: &str, port: u16) -> Result<(), OpenShellError> {
        self.run(
            &format!("stop forwarding port {port} to “{name}”"),
            &["forward", "stop", &port.to_string(), name],
        )
        .map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::{Forward, Forwards, ListedForward};
    use crate::openshell::{GATEWAY_VARIABLE, Gateway, OpenShell};

    #[test]
    fn a_forward_listed_by_openshell_is_read_without_its_alive() {
        let listed: Vec<ListedForward> = serde_json::from_str(
            r#"[{"alive":false,"bind_address":"127.0.0.1","pid":66003,"port":3000,"sandbox":"web","workspace":"default"}]"#,
        )
        .unwrap();
        assert_eq!(
            listed,
            [ListedForward {
                sandbox: "web".to_string(),
                port: 3000
            }]
        );
    }

    #[test]
    fn forwards_openshell_no_longer_runs_are_forgotten_and_each_is_kept_with_its_gateway() {
        let forward = |gateway: &str, sandbox: &str, port| Forward {
            gateway: gateway.to_string(),
            sandbox: sandbox.to_string(),
            port,
        };
        let mut forwards = Forwards {
            forwards: vec![
                forward("microvm", "web", 3000),
                forward("openshell", "web", 3001),
                forward("openshell", "gone", 8080),
            ],
        };
        forwards.keep_running(&[
            ListedForward {
                sandbox: "web".to_string(),
                port: 3000,
            },
            ListedForward {
                sandbox: "web".to_string(),
                port: 3001,
            },
        ]);
        assert_eq!(forwards.forwards.len(), 2);
        let on_microvm: Vec<u16> = forwards
            .of("microvm", "web")
            .map(|forward| forward.port)
            .collect();
        assert_eq!(on_microvm, [3000]);
    }

    #[test]
    fn a_forward_is_started_on_the_sandboxs_own_gateway() {
        let openshell = OpenShell::new("/opt/homebrew/bin/openshell").on(Gateway::named("microvm"));
        let command = openshell.command(&["forward", "start", "3000", "web", "--background"], None);
        let gateway = command
            .get_envs()
            .find(|(name, _)| *name == GATEWAY_VARIABLE)
            .and_then(|(_, value)| value.map(|value| value.to_string_lossy().into_owned()));
        assert_eq!(gateway.as_deref(), Some("microvm"));
    }
}
