//! A sandbox made for Claude Code on a real OpenShell gateway, with a real provider: its image built, a folder
//! copied in, and Claude Code answering from inside it, before and after the sandbox is stopped and started again,
//! then the sandbox deleted.
//!
//! It needs OpenShell's gateway, Docker and a provider holding a Claude key, so it's left out of `cargo test`:
//! `PLAYPEN_TEST_PROVIDER=<provider> cargo test -p playpen-core --test openshell -- --ignored`.

use std::process::Command;
use std::time::{Duration, Instant};

use playpen_core::{Agent, Docker, OpenShell, Phase, Plan, Sandbox};

/// The sandbox `name` once it's in `phase`, waiting up to five minutes for it, since a cluster pulls images first.
fn once_in(openshell: &OpenShell, name: &str, phase: Phase) -> Result<Sandbox, String> {
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        let found = openshell
            .sandboxes()
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|sandbox| sandbox.name == name);
        match found {
            Some(sandbox) if sandbox.phase == phase => return Ok(sandbox),
            found if Instant::now() > deadline => return Err(format!("not {phase:?}: {found:?}")),
            _ => std::thread::sleep(Duration::from_secs(2)),
        }
    }
}

const QUESTION: &str = "Read NOTES.md and reply with only the password in it.";

/// Asks the agent in the sandbox for the password in the folder copied in, and gives its answer.
fn ask(openshell: &OpenShell, plan: &Plan) -> String {
    let codex = format!(
        "node /sandbox/.codex/playpen-sign-in.js; exec codex exec --skip-git-repo-check '{QUESTION}'"
    );
    let command: Vec<&str> = match plan.agent {
        Agent::ClaudeCode => vec!["claude", "-p", QUESTION],
        Agent::Codex => vec!["sh", "-c", &codex],
        Agent::Pi | Agent::Omp | Agent::Pig => vec![plan.agent.command(), "-p", QUESTION],
    };
    let output = Command::new(openshell.binary())
        .args([
            "sandbox",
            "exec",
            "--name",
            &plan.name,
            "--no-tty",
            "--workdir",
            &plan.workdir(),
            "--",
        ])
        .args(&command)
        .output()
        .unwrap();
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
#[ignore = "needs OpenShell, Docker and a Claude provider: PLAYPEN_TEST_PROVIDER"]
fn claude_code_answers_from_a_sandbox_playpen_made() {
    let provider =
        std::env::var("PLAYPEN_TEST_PROVIDER").expect("PLAYPEN_TEST_PROVIDER names a provider");
    let openshell = OpenShell::find().unwrap();
    let docker = Docker::find().unwrap();
    let folder = std::env::temp_dir()
        .join(format!("playpen-test-{}", std::process::id()))
        .join("Project");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("NOTES.md"), "The password is periwinkle.\n").unwrap();
    let plan = Plan {
        name: format!("playpen-test-{}", std::process::id()),
        agent: Agent::ClaudeCode,
        provider: Some(provider),
        model: None,
        account: None,
        services: Vec::new(),
        variables: Vec::new(),
        setups: Vec::new(),
        folders: vec![folder],
    };
    let steps = std::sync::Mutex::new(Vec::new());
    let made = plan.make(&openshell, &docker, |step| steps.lock().unwrap().push(step));
    // Everything is tried before anything is checked, so the sandbox is deleted whatever fails.
    let tried = made.map_err(|error| error.to_string()).and_then(|()| {
        let sandbox = once_in(&openshell, &plan.name, Phase::Ready)?;
        let first = ask(&openshell, &plan);
        openshell
            .stop(&plan.name)
            .map_err(|error| error.to_string())?;
        once_in(&openshell, &plan.name, Phase::Stopped)?;
        openshell
            .start(&plan.name)
            .map_err(|error| error.to_string())?;
        once_in(&openshell, &plan.name, Phase::Ready)?;
        let again = ask(&openshell, &plan);
        Ok((sandbox, first, again))
    });
    let _ = openshell.delete(&plan.name);

    let (sandbox, first, again) = tried.unwrap();
    assert_eq!(sandbox.agent.as_deref(), Some("claude-code"));
    assert_eq!(sandbox.workdir.as_deref(), Some("/sandbox/Project"));
    assert!(first.to_lowercase().contains("periwinkle"), "{first}");
    assert!(
        again.to_lowercase().contains("periwinkle"),
        "the folder and the key are still there after a restart: {again}"
    );
    eprintln!("steps: {:?}", steps.lock().unwrap());
}

/// What `command` printed, run in the sandbox on `openshell`'s gateway.
fn run_in(openshell: &OpenShell, name: &str, workdir: &str, command: &[&str]) -> String {
    let mut line = Command::new(openshell.binary());
    if let Some(gateway) = openshell.gateway().name() {
        line.env(playpen_core::GATEWAY_VARIABLE, gateway);
    }
    let output = line
        .args([
            "sandbox",
            "exec",
            "--name",
            name,
            "--no-tty",
            "--workdir",
            workdir,
            "--",
        ])
        .args(command)
        .output()
        .unwrap();
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
#[ignore = "needs a second OpenShell gateway, Docker it shares, and no model: PLAYPEN_TEST_GATEWAY"]
fn a_sandbox_is_made_on_a_named_gateway_and_is_not_on_the_other() {
    let gateway =
        std::env::var("PLAYPEN_TEST_GATEWAY").expect("PLAYPEN_TEST_GATEWAY names a gateway");
    let active = OpenShell::find().unwrap();
    let openshell = OpenShell::find()
        .unwrap()
        .on(playpen_core::Gateway::named(gateway));
    let docker = Docker::find().unwrap();
    let folder = std::env::temp_dir()
        .join(format!("playpen-gateway-test-{}", std::process::id()))
        .join("Project");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("NOTES.md"), "The password is periwinkle.\n").unwrap();
    let plan = Plan {
        // Short, since a Kubernetes gateway takes names of up to 19 characters where a Docker one takes 63.
        name: format!("pp-gw-{}", std::process::id()),
        agent: Agent::ClaudeCode,
        provider: None,
        model: None,
        account: None,
        services: Vec::new(),
        variables: Vec::new(),
        setups: Vec::new(),
        folders: vec![folder],
    };
    let steps = std::sync::Mutex::new(Vec::new());
    let made = plan.make(&openshell, &docker, |step| steps.lock().unwrap().push(step));
    // Everything is tried before anything is checked, so the sandbox is deleted whatever fails.
    let tried = made.map_err(|error| error.to_string()).and_then(|()| {
        let sandbox = once_in(&openshell, &plan.name, Phase::Ready)?;
        let notes = run_in(
            &openshell,
            &plan.name,
            &plan.workdir(),
            &["cat", "NOTES.md"],
        );
        let elsewhere = active
            .sandboxes()
            .map_err(|error| error.to_string())?
            .iter()
            .any(|other| other.name == plan.name);
        openshell
            .stop(&plan.name)
            .map_err(|error| error.to_string())?;
        once_in(&openshell, &plan.name, Phase::Stopped)?;
        openshell
            .start(&plan.name)
            .map_err(|error| error.to_string())?;
        once_in(&openshell, &plan.name, Phase::Ready)?;
        let again = run_in(
            &openshell,
            &plan.name,
            &plan.workdir(),
            &["cat", "NOTES.md"],
        );
        Ok((sandbox, notes, elsewhere, again))
    });
    let _ = openshell.delete(&plan.name);

    let (sandbox, notes, elsewhere, again) = tried.unwrap();
    assert_eq!(sandbox.workdir.as_deref(), Some("/sandbox/Project"));
    assert!(
        notes.contains("periwinkle"),
        "the folder was copied in: {notes}"
    );
    assert!(!elsewhere, "the other gateway doesn't have it");
    assert!(
        again.contains("periwinkle"),
        "the folder is still there after a stop and start: {again}"
    );
    eprintln!("steps: {:?}", steps.lock().unwrap());
}

#[test]
#[ignore = "needs OpenShell, Docker and a ChatGPT provider Playpen signed in: PLAYPEN_TEST_CHATGPT_PROVIDER"]
fn codex_answers_on_a_chatgpt_subscription_from_a_sandbox_playpen_made() {
    let provider = std::env::var("PLAYPEN_TEST_CHATGPT_PROVIDER")
        .expect("PLAYPEN_TEST_CHATGPT_PROVIDER names a provider");
    let accounts = std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
        .join("Library/Application Support/tech.phin.playpen/chatgpt");
    let account = playpen_core::Account::load(&accounts, &provider).unwrap();
    let openshell = OpenShell::find().unwrap();
    let docker = Docker::find().unwrap();
    let folder = std::env::temp_dir()
        .join(format!("pp-codex-{}", std::process::id()))
        .join("Project");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("NOTES.md"), "The password is periwinkle.\n").unwrap();
    let plan = Plan {
        name: format!("pp-codex-{}", std::process::id()),
        agent: Agent::Codex,
        provider: Some(provider),
        model: None,
        account: Some(account),
        services: Vec::new(),
        variables: Vec::new(),
        setups: Vec::new(),
        folders: vec![folder],
    };
    let made = plan.make(&openshell, &docker, |_| {});
    let tried = made.map_err(|error| error.to_string()).and_then(|()| {
        once_in(&openshell, &plan.name, Phase::Ready)?;
        let first = ask(&openshell, &plan);
        openshell
            .stop(&plan.name)
            .map_err(|error| error.to_string())?;
        once_in(&openshell, &plan.name, Phase::Stopped)?;
        openshell
            .start(&plan.name)
            .map_err(|error| error.to_string())?;
        once_in(&openshell, &plan.name, Phase::Ready)?;
        let again = ask(&openshell, &plan);
        Ok((first, again))
    });
    let _ = openshell.delete(&plan.name);

    let (first, again) = tried.unwrap();
    assert!(first.to_lowercase().contains("periwinkle"), "{first}");
    assert!(
        again.to_lowercase().contains("periwinkle"),
        "still signed in after a restart: {again}"
    );
}

#[test]
#[ignore = "needs OpenShell, Docker and an API-key provider: PLAYPEN_TEST_PI_PROVIDER, and PLAYPEN_TEST_PI_MODEL for LiteLLM"]
fn pi_omp_and_pig_each_answer_on_an_api_key_from_a_sandbox_playpen_made() {
    let provider = std::env::var("PLAYPEN_TEST_PI_PROVIDER")
        .expect("PLAYPEN_TEST_PI_PROVIDER names a provider");
    let id = std::env::var("PLAYPEN_TEST_PI_MODEL").ok();
    let openshell = OpenShell::find().unwrap();
    let docker = Docker::find().unwrap();
    let listed = openshell.providers().unwrap();
    let held = listed
        .iter()
        .find(|listed| listed.name == provider)
        .expect("the provider is on the active gateway");
    let profiles = openshell.profiles().unwrap();
    let model = playpen_core::Model::of(held, &profiles, id).expect("a kind Playpen adds");
    let mut failed = Vec::new();
    for agent in [Agent::Pi, Agent::Omp, Agent::Pig] {
        let folder = std::env::temp_dir()
            .join(format!("pp-{}-{}", agent.id(), std::process::id()))
            .join("Project");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("NOTES.md"), "The password is periwinkle.\n").unwrap();
        let plan = Plan {
            name: format!("pp-{}-{}", agent.id(), std::process::id()),
            agent,
            provider: Some(provider.clone()),
            model: Some(model.clone()),
            account: None,
            services: Vec::new(),
            variables: Vec::new(),
            setups: Vec::new(),
            folders: vec![folder],
        };
        let made = plan.make(&openshell, &docker, |_| {});
        let tried = made.map_err(|error| error.to_string()).and_then(|()| {
            once_in(&openshell, &plan.name, Phase::Ready)?;
            Ok(ask(&openshell, &plan))
        });
        let _ = openshell.delete(&plan.name);
        match tried {
            Ok(answer) if answer.to_lowercase().contains("periwinkle") => {}
            Ok(answer) => failed.push(format!("{}: {answer}", agent.name())),
            Err(error) => failed.push(format!("{}: {error}", agent.name())),
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}
