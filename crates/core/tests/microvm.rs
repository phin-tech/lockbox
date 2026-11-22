//! The MicroVM gateway Playpen sets up on this Mac: set up from nothing, a Claude Code sandbox made on it the way New
//! Agent makes one, set up again with another size over that sandbox, and removed. It starts a real launch agent and registers a real gateway, so it's left out of
//! `cargo test`, and won't run over one that's set up already: `just microvm-test`.

use std::time::{Duration, Instant};

use playpen_core::{
    Agent, Docker, Gateway, MICROVM_GATEWAY, MicroVm, MicroVmSize, OpenShell, Phase, Plan,
};

#[test]
#[ignore = "needs OpenShell's VM driver, e2fsprogs and Docker; sets up and removes the MicroVM gateway"]
fn a_microvm_gateway_is_set_up_runs_an_agents_sandbox_and_is_removed() {
    let openshell = OpenShell::find().unwrap();
    let microvm = MicroVm::find(&openshell).unwrap();
    assert!(microvm.missing().is_empty(), "{:?}", microvm.missing());
    assert!(
        !microvm.is_set_up(),
        "a MicroVM gateway is set up already; this test won't touch it"
    );
    // The Homebrew gateway's client certificates, which setting up must leave as they are.
    let theirs = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap()
        .join(".config/openshell/gateways/openshell/mtls");
    let read_theirs =
        || ["ca.crt", "tls.crt", "tls.key"].map(|file| std::fs::read(theirs.join(file)).ok());
    let before = read_theirs();
    let steps = std::sync::Mutex::new(Vec::new());
    let set_up = microvm.set_up(MicroVmSize::default(), |step| {
        steps.lock().unwrap().push(step);
    });
    let on_it = openshell.clone().on(Gateway::named(MICROVM_GATEWAY));
    let plan = Plan {
        name: format!("pp-microvm-{}", std::process::id()),
        agent: Agent::ClaudeCode,
        provider: None,
        account: None,
        services: Vec::new(),
        variables: Vec::new(),
        setups: Vec::new(),
        folders: Vec::new(),
    };
    // Everything is tried before anything is checked, so the gateway is removed whatever fails.
    let tried = set_up
        .map_err(|error| error.to_string())
        .and_then(|()| {
            plan.make(&on_it, &Docker::find().unwrap(), |_| {})
                .map_err(|error| error.to_string())
        })
        .and_then(|()| {
            let deadline = Instant::now() + Duration::from_secs(180);
            loop {
                let found = on_it
                    .sandboxes()
                    .map_err(|error| error.to_string())?
                    .into_iter()
                    .find(|sandbox| sandbox.name == plan.name);
                match found {
                    Some(sandbox) if sandbox.phase == Phase::Ready => return Ok(sandbox),
                    found if Instant::now() > deadline => {
                        return Err(format!("not ready: {found:?}"));
                    }
                    _ => std::thread::sleep(Duration::from_secs(2)),
                }
            }
        });
    let gateways = openshell.gateways().unwrap_or_default();
    // Setting it up again, with another size, restarts it over a sandbox that has something in it.
    let shell = |script: &str| {
        std::process::Command::new(openshell.binary())
            .args([
                "-g",
                MICROVM_GATEWAY,
                "sandbox",
                "exec",
                &plan.name,
                "--no-tty",
                "--no-login-shell",
                "--",
                "sh",
                "-c",
                script,
            ])
            .output()
            .map(|output| {
                format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout).trim(),
                    String::from_utf8_lossy(&output.stderr).trim()
                )
            })
            .unwrap_or_default()
    };
    let again = tried.as_ref().ok().map(|_| {
        // Flushed, since what a microVM hasn't written to its disk is lost when it stops, as after a power cut.
        shell("echo kept > /sandbox/kept.txt && sync");
        let smaller = MicroVmSize {
            cpus: 2,
            ..MicroVmSize::default()
        };
        let done = microvm.set_up(smaller, |_| {});
        let deadline = Instant::now() + Duration::from_secs(120);
        let after = loop {
            let after = shell("cat /sandbox/kept.txt; nproc");
            if after.starts_with("kept") || Instant::now() > deadline {
                break after;
            }
            std::thread::sleep(Duration::from_secs(3));
        };
        (done, after, microvm.configured_size())
    });
    let _ = on_it.delete(&plan.name);
    let kept = microvm.remove();
    assert!(
        before == read_theirs(),
        "the openshell gateway's certificates are untouched"
    );
    let sandbox = tried.unwrap();
    assert_eq!(sandbox.agent.as_deref(), Some(Agent::ClaudeCode.id()));
    assert!(
        gateways
            .iter()
            .any(|gateway| gateway.name == MICROVM_GATEWAY && gateway.is_local()),
        "registered as a gateway on this Mac: {gateways:?}"
    );
    assert!(
        gateways
            .iter()
            .find(|gateway| gateway.active)
            .is_none_or(|gateway| gateway.name != MICROVM_GATEWAY),
        "the active gateway is left as it was"
    );
    assert!(
        kept.unwrap().join("tls/ca.crt").is_file(),
        "its folder stays"
    );
    assert!(!microvm.is_set_up());
    assert!(
        !openshell
            .gateways()
            .unwrap()
            .iter()
            .any(|gateway| gateway.name == MICROVM_GATEWAY),
        "OpenShell forgot it"
    );
    assert_eq!(steps.into_inner().unwrap().len(), 3);
    let (done, after, size) = again.unwrap();
    assert_eq!(done, Ok(()), "set up again over itself");
    assert_eq!(
        after, "kept\n2",
        "the sandbox came back with its disk, and the new size"
    );
    assert_eq!(size.map(|size| size.cpus), Some(2));
}

/// Codex is set up in its sandbox by a command run there, which has to go to the gateway the sandbox is on: the
/// MicroVM gateway is never the active one.
#[test]
#[ignore = "needs the MicroVM gateway set up, with a ChatGPT provider called chatgpt that Playpen signed in"]
fn a_codex_sandbox_on_the_microvm_gateway_is_set_up_there_and_kept() {
    let openshell = OpenShell::find()
        .unwrap()
        .on(Gateway::named(MICROVM_GATEWAY));
    let accounts = std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
        .join("Library/Application Support/tech.phin.playpen/chatgpt");
    let plan = Plan {
        name: format!("pp-codex-{}", std::process::id()),
        agent: Agent::Codex,
        provider: Some("chatgpt".to_string()),
        account: Some(playpen_core::Account::load(&accounts, "chatgpt").unwrap()),
        services: Vec::new(),
        variables: Vec::new(),
        setups: Vec::new(),
        folders: Vec::new(),
    };
    let made = plan.make(&openshell, &Docker::find().unwrap(), |_| {});
    let sandbox = openshell
        .sandboxes()
        .unwrap()
        .into_iter()
        .find(|sandbox| sandbox.name == plan.name);
    // The session Playpen opens, in a terminal of its own for long enough to start, then stopped.
    let said = sandbox.as_ref().map(|sandbox| {
        let line = playpen_core::open_command(
            openshell.binary(),
            openshell.gateway(),
            sandbox,
            playpen_core::What::Agent,
        );
        let mut session = std::process::Command::new("/usr/bin/script")
            .args(["-q", "/dev/null", "/bin/sh", "-c", &line])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut out = session.stdout.take().unwrap();
        let reading = std::thread::spawn(move || {
            let mut printed = Vec::new();
            let _ = std::io::Read::read_to_end(&mut out, &mut printed);
            String::from_utf8_lossy(&printed).into_owned()
        });
        std::thread::sleep(Duration::from_secs(20));
        let ended_early = session.try_wait().unwrap().is_some();
        let _ = std::process::Command::new("/usr/bin/pkill")
            .args(["-KILL", "-P", &session.id().to_string()])
            .status();
        let _ = session.kill();
        let _ = session.wait();
        (ended_early, reading.join().unwrap())
    });
    let _ = openshell.delete(&plan.name);
    made.unwrap();
    let (ended_early, printed) = said.expect("it's still there");
    assert!(!ended_early, "Codex is still running: {printed}");
    assert!(printed.contains("OpenAI Codex"), "{printed}");
    assert!(
        !printed.contains("Sign in with ChatGPT"),
        "signed in already"
    );
}
