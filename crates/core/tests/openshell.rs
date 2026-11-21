//! A sandbox made for Claude Code on a real OpenShell gateway, with a real provider: its image built, a folder
//! copied in, and Claude Code answering from inside it, before and after the sandbox is stopped and started again,
//! then the sandbox deleted.
//!
//! It needs OpenShell's gateway, Docker and a provider holding a Claude key, so it's left out of `cargo test`:
//! `PLAYPEN_TEST_PROVIDER=<provider> cargo test -p playpen-core --test openshell -- --ignored`.

use std::process::Command;
use std::time::{Duration, Instant};

use playpen_core::{Agent, Docker, OpenShell, Phase, Plan, Sandbox};

/// The sandbox `name` once it's in `phase`, waiting up to a minute for it.
fn once_in(openshell: &OpenShell, name: &str, phase: Phase) -> Result<Sandbox, String> {
    let deadline = Instant::now() + Duration::from_secs(60);
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

/// Asks Claude Code in the sandbox for the password in the folder copied in, and gives its answer.
fn ask(openshell: &OpenShell, plan: &Plan) -> String {
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
        .args([
            "claude",
            "-p",
            "Read NOTES.md and reply with only the password in it.",
        ])
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
        folder: Some(folder),
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
