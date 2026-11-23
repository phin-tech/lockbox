//! A service attached to sandboxes on a real OpenShell gateway: a throwaway profile and provider for
//! postman-echo.com, read-only, attached when a sandbox is made and to one already running, then detached. The agent
//! sees a stand-in, the service gets the real key, a write is refused, and detaching closes the host again. It needs
//! OpenShell's gateway, Docker and the internet, so it's left out of `cargo test`: `just services-test`.

use std::process::Command;
use std::time::{Duration, Instant};

use playpen_core::{Agent, Docker, OpenShell, Phase, Plan, Service};

const PROFILE: &str = r#"id: pp-test-echo
display_name: "Echo test"
description: "Throwaway, for Playpen's services test"
category: other
credentials:
  - name: token
    description: "test token"
    env_vars: [ECHO_TOKEN]
    required: true
    auth_style: bearer
    header_name: authorization
endpoints:
  - host: postman-echo.com
    port: 443
    protocol: rest
    access: read-only
    enforcement: enforce
"#;

/// What a GET and a POST to the echo service say, from node in the sandbox: whether the real key arrived.
const PROBE: &str = r#"
const headers = {authorization: "Bearer " + process.env.ECHO_TOKEN};
fetch("https://postman-echo.com/get", {headers}).then(r => r.json())
  .then(j => console.log("GET", j.headers.authorization === "Bearer pp-secret-123" ? "real" : "stand-in"))
  .catch(e => console.log("GET blocked"))
  .then(() => fetch("https://postman-echo.com/post", {method: "POST", body: "x", headers}))
  .then(r => console.log("POST", r.status)).catch(() => console.log("POST blocked"));
"#;

fn once_ready(openshell: &OpenShell, name: &str) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let ready = openshell
            .sandboxes()
            .map_err(|error| error.to_string())?
            .into_iter()
            .any(|sandbox| sandbox.name == name && sandbox.phase == Phase::Ready);
        if ready {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(format!("{name} isn't ready"));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn exec(openshell: &OpenShell, name: &str, command: &[&str]) -> String {
    let output = Command::new(openshell.binary())
        .args(["sandbox", "exec", "--name", name, "--no-tty", "--"])
        .args(command)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn plan(name: String, services: Vec<Service>) -> Plan {
    Plan {
        name,
        agent: Agent::ClaudeCode.into(),
        image: None,
        provider: None,
        model: None,
        account: None,
        services,
        variables: Vec::new(),
        setups: Vec::new(),
        folders: Vec::new(),
    }
}

#[test]
#[ignore = "needs OpenShell's gateway, Docker and the internet"]
fn a_service_reaches_only_its_host_with_its_access_and_detaching_closes_it() {
    let openshell = OpenShell::find().unwrap();
    let docker = Docker::find().unwrap();
    let pid = std::process::id();
    let profile_file = std::env::temp_dir().join(format!("pp-echo-{pid}.yaml"));
    std::fs::write(&profile_file, PROFILE).unwrap();
    let gateway = |args: &[&str]| {
        Command::new(openshell.binary())
            .args(args)
            .env("ECHO_TOKEN", "pp-secret-123")
            .output()
            .unwrap()
    };
    gateway(&[
        "profile",
        "import",
        "--global",
        "-f",
        &profile_file.to_string_lossy(),
    ]);
    let provider = format!("pp-echo-{pid}");
    gateway(&[
        "provider",
        "create",
        "--name",
        &provider,
        "--type",
        "pp-test-echo",
        "--credential",
        "ECHO_TOKEN",
    ]);

    let tried = (|| {
        let providers = openshell.providers().map_err(|error| error.to_string())?;
        let profiles = openshell.profiles().map_err(|error| error.to_string())?;
        let (found, profile) = Service::choices(&providers, &profiles)
            .into_iter()
            .find(|(found, _)| found.name == provider)
            .ok_or("the test provider isn't offered as a service")?;
        let service = Service::new(found, profile);

        // Attached when it's made.
        let made = plan(format!("pp-svc-a-{pid}"), vec![service.clone()]);
        made.make(&openshell, &docker, |_| {})
            .map_err(|error| error.to_string())?;
        once_ready(&openshell, &made.name)?;
        let stand_in = exec(&openshell, &made.name, &["sh", "-c", "echo $ECHO_TOKEN"]);
        let at_make = exec(&openshell, &made.name, &["node", "-e", PROBE]);

        // Attached while it runs, then detached.
        let running = plan(format!("pp-svc-b-{pid}"), Vec::new());
        running
            .make(&openshell, &docker, |_| {})
            .map_err(|error| error.to_string())?;
        once_ready(&openshell, &running.name)?;
        let before = exec(&openshell, &running.name, &["node", "-e", PROBE]);
        openshell
            .attach_service(&running.name, &service)
            .map_err(|error| error.to_string())?;
        let attached = openshell
            .attached(&running.name)
            .map_err(|error| error.to_string())?;
        let while_attached = exec(&openshell, &running.name, &["node", "-e", PROBE]);
        openshell
            .detach_service(&running.name, &service)
            .map_err(|error| error.to_string())?;
        let after = exec(&openshell, &running.name, &["node", "-e", PROBE]);
        Ok::<_, String>((stand_in, at_make, before, attached, while_attached, after))
    })();

    let _ = openshell.delete(&format!("pp-svc-a-{pid}"));
    let _ = openshell.delete(&format!("pp-svc-b-{pid}"));
    std::thread::sleep(Duration::from_secs(5));
    gateway(&["provider", "delete", &provider]);
    gateway(&["profile", "delete", "pp-test-echo", "--global"]);
    let _ = std::fs::remove_file(&profile_file);

    let (stand_in, at_make, before, attached, while_attached, after) = tried.unwrap();
    assert!(
        stand_in.starts_with("openshell:resolve:"),
        "a stand-in: {stand_in}"
    );
    assert_eq!(
        at_make, "GET real\nPOST 403",
        "read-only, with the real key"
    );
    assert_eq!(before, "GET blocked\nPOST blocked");
    assert!(attached.iter().any(|found| found.name == provider));
    assert_eq!(while_attached, "GET real\nPOST 403");
    assert_eq!(after, "GET blocked\nPOST blocked", "closed again");
}

#[test]
#[ignore = "needs OpenShell's gateway"]
fn a_service_playpen_adds_is_offered_as_one_and_goes_with_its_profile() {
    use playpen_core::{Access, Key, KeySource, NewService};

    let openshell = OpenShell::find().unwrap();
    let name = format!("pp-svc-{}", std::process::id());
    let new = NewService::new(&name, "postman-echo.com", "ECHO_TOKEN", Access::ReadOnly).unwrap();
    let added = openshell.add_service(&new, &KeySource::Key(Key::new("pp-secret-123")), None);
    let found = (|| {
        let providers = openshell.providers().map_err(|error| error.to_string())?;
        let profiles = openshell.profiles().map_err(|error| error.to_string())?;
        let offered = Service::choices(&providers, &profiles)
            .into_iter()
            .find(|(provider, _)| provider.name == name)
            .map(|(provider, profile)| Service::new(provider, profile));
        Ok::<_, String>((
            providers.into_iter().find(|provider| provider.name == name),
            offered,
        ))
    })();
    if let Ok((Some(provider), _)) = &found {
        openshell.delete_provider(provider).unwrap();
    }
    let profile_left = openshell
        .profiles()
        .unwrap()
        .into_iter()
        .any(|profile| profile.id == new.profile_id());
    added.unwrap();
    let (provider, offered) = found.unwrap();
    assert!(provider.is_some());
    let offered = offered.expect("offered as a service");
    assert_eq!(offered.endpoints[0].host, "postman-echo.com");
    assert_eq!(offered.endpoints[0].access, "read-only");
    assert!(!profile_left, "its profile goes with it");
}

#[test]
#[ignore = "needs OpenShell's gateway"]
fn a_model_key_from_a_env_file_is_added_as_a_model_key() {
    use playpen_core::{Key, KeySource, Kind, NewProvider};

    let openshell = OpenShell::find().unwrap();
    let name = format!("pp-model-{}", std::process::id());
    let new = NewProvider::new(Kind::Anthropic, &name, "").unwrap();
    let added = openshell.add_provider_from(&new, &KeySource::Key(Key::new("sk-ant-test")), None);
    let found = openshell
        .providers()
        .unwrap()
        .into_iter()
        .find(|provider| provider.name == name);
    if let Some(provider) = &found {
        openshell.delete_provider(provider).unwrap();
    }
    added.unwrap();
    assert_eq!(
        found.map(|provider| provider.kind).as_deref(),
        Some("playpen-anthropic")
    );
}

#[test]
#[ignore = "needs OpenShell's gateway, Docker and the Keychain"]
fn a_kits_variables_reach_a_new_sandbox_with_a_keychain_secret_fetched_around_openshell() {
    use playpen_core::{Variable, VariableValue};

    let openshell = OpenShell::find().unwrap();
    let docker = Docker::find().unwrap();
    let pid = std::process::id();
    let item = format!("playpen-test-{pid}");
    // Made by `security`, so `security` may read it back without asking.
    Command::new("/usr/bin/security")
        .args([
            "add-generic-password",
            "-s",
            &item,
            "-a",
            "playpen",
            "-w",
            "kc-secret-456",
        ])
        .output()
        .unwrap();
    let plan = Plan {
        variables: vec![
            Variable {
                name: "NODE_ENV".to_string(),
                value: VariableValue::Value("development".to_string()),
            },
            Variable {
                name: "FROM_KEYCHAIN".to_string(),
                value: VariableValue::Keychain(item.clone()),
            },
        ],
        ..plan(format!("pp-vars-{pid}"), Vec::new())
    };
    let tried = plan
        .make(&openshell, &docker, |_| {})
        .map_err(|error| error.to_string())
        .and_then(|()| {
            once_ready(&openshell, &plan.name)?;
            Ok(exec(
                &openshell,
                &plan.name,
                &["sh", "-c", "echo $NODE_ENV $FROM_KEYCHAIN"],
            ))
        });
    let _ = openshell.delete(&plan.name);
    Command::new("/usr/bin/security")
        .args(["delete-generic-password", "-s", &item])
        .output()
        .unwrap();
    assert_eq!(tried.unwrap(), "development kc-secret-456");
}
