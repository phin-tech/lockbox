//! Folders copied into a sandbox Playpen made, at the start and while it runs, on a real OpenShell gateway: they land
//! side by side where the agent starts, and one of a name that's there already is refused. It needs no provider, but
//! does need OpenShell's gateway and Docker, so it's left out of `cargo test`: `just folders-test`.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use playpen_core::{AddError, AddedFolders, Agent, Docker, OpenShell, Phase, Plan, Sandbox};

/// The sandbox `name` once it's ready, waiting up to a minute for it.
fn once_ready(openshell: &OpenShell, name: &str) -> Result<Sandbox, String> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let found = openshell
            .sandboxes()
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|sandbox| sandbox.name == name);
        match found {
            Some(sandbox) if sandbox.phase == Phase::Ready => return Ok(sandbox),
            found if Instant::now() > deadline => return Err(format!("not ready: {found:?}")),
            _ => std::thread::sleep(Duration::from_secs(2)),
        }
    }
}

/// A folder `name` holding a file that says its name, under `root`.
fn folder(root: &std::path::Path, name: &str) -> PathBuf {
    let folder = root.join(name);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("name.txt"), format!("{name}\n")).unwrap();
    folder
}

#[test]
#[ignore = "needs OpenShell's gateway and Docker"]
fn folders_go_in_side_by_side_when_its_made_and_while_it_runs() {
    let openshell = OpenShell::find().unwrap();
    let docker = Docker::find().unwrap();
    let root = std::env::temp_dir().join(format!("pp-folders-{}", std::process::id()));
    let notes = root.join("notes");
    let plan = Plan {
        name: format!("pp-folders-{}", std::process::id()),
        agent: Agent::ClaudeCode,
        provider: None,
        model: None,
        account: None,
        services: Vec::new(),
        variables: Vec::new(),
        setups: Vec::new(),
        folders: vec![folder(&root, "Alpha"), folder(&root, "Beta")],
    };
    let added = AddedFolders::new(&notes);
    // Everything is tried before anything is checked, so the sandbox is deleted whatever fails.
    let tried = plan
        .make(&openshell, &docker, |_| {})
        .map_err(|error| error.to_string())
        .and_then(|()| {
            let sandbox = once_ready(&openshell, &plan.name)?;
            added
                .add(&openshell, &sandbox, &[folder(&root, "Gamma")])
                .map_err(|error| error.to_string())?;
            let again = added.add(&openshell, &sandbox, &[folder(&root.join("other"), "Beta")]);
            let listing = Command::new(openshell.binary())
                .args(["sandbox", "exec", "--name", &plan.name, "--no-tty"])
                .args(["--workdir", &plan.workdir(), "--"])
                .args(["sh", "-c", "cat */name.txt"])
                .output()
                .unwrap();
            let mut listed = vec![sandbox];
            added.fill(&mut listed);
            Ok((
                again,
                String::from_utf8_lossy(&listing.stdout).into_owned(),
                listed.remove(0).folders,
            ))
        });
    let _ = openshell.delete(&plan.name);
    let _ = std::fs::remove_dir_all(&root);
    let (again, names, folders) = tried.unwrap();
    assert_eq!(names, "Alpha\nBeta\nGamma\n");
    assert!(
        matches!(again, Err(AddError::Taken(name)) if name == "Beta"),
        "a folder of a name that's there"
    );
    assert_eq!(folders.len(), 3, "{folders:?}");
    assert!(folders[2].ends_with("/Gamma"), "{folders:?}");
}
