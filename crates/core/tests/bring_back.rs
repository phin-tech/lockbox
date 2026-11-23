//! Work brought back from a sandbox on a real OpenShell gateway: a repository with uncommitted edits of its own is
//! copied in with its history, the "agent" commits, edits, deletes and adds a binary, and the work comes back as a
//! branch, leaving the person's branch, index and files as they were. It needs OpenShell's gateway and Docker, so it's
//! left out of `cargo test`: `just bring-back-test`.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use playpen_core::{Agent, Docker, OpenShell, Phase, Plan, Repo, bring_back};

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn once_ready(openshell: &OpenShell, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !openshell
        .sandboxes()
        .unwrap()
        .iter()
        .any(|sandbox| sandbox.name == name && sandbox.phase == Phase::Ready)
    {
        assert!(Instant::now() < deadline, "{name} isn't ready");
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// What the "agent" does: a commit, then an edit, a deletion, a binary and something ignored, left uncommitted.
const AGENT: &str = r#"set -e
cd /sandbox/Shop
git log --oneline | wc -l > /sandbox/history
echo "agent line" >> README.md
git -c user.name=agent -c user.email=agent@sandbox commit -qam "Agent: a line in the README"
rm LICENSE
head -c 3000 /dev/urandom > logo.bin
mkdir -p node_modules && echo junk > node_modules/ignored
"#;

#[test]
#[ignore = "needs OpenShell's gateway and Docker"]
fn work_comes_back_as_a_branch_without_the_persons_own_edits_and_touching_nothing_of_theirs() {
    let openshell = OpenShell::find().unwrap();
    let docker = Docker::find().unwrap();
    let pid = std::process::id();
    let root = std::env::temp_dir().join(format!("pp-bring-{pid}"));
    let repo = root.join("Shop");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "shop\n").unwrap();
    std::fs::write(repo.join("LICENSE"), "MIT\n").unwrap();
    std::fs::write(repo.join(".gitignore"), "node_modules/\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=me",
            "-c",
            "user.email=me@mac",
            "commit",
            "-qm",
            "First",
        ],
    );
    // The person's own work, not yet committed, which the agent is given but which isn't the agent's.
    std::fs::write(repo.join("README.md"), "shop\nmine\n").unwrap();
    std::fs::write(repo.join("notes.txt"), "my notes\n").unwrap();
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let status = git(&repo, &["status", "--porcelain"]);

    let name = format!("pp-back-{pid}");
    let plan = Plan {
        name: name.clone(),
        agent: Agent::ClaudeCode,
        provider: None,
        model: None,
        account: None,
        services: Vec::new(),
        variables: Vec::new(),
        setups: Vec::new(),
        folders: vec![repo.clone()],
    };
    let tried = (|| {
        plan.make(&openshell, &docker, |_| {})
            .map_err(|error| error.to_string())?;
        once_ready(&openshell, &name);
        let given = openshell
            .exec_bytes(
                &name,
                "read",
                "cat /sandbox/Shop/notes.txt; git -C /sandbox/Shop status --porcelain",
                &[],
            )
            .map_err(|error| error.to_string())?;
        openshell
            .exec_bytes(&name, "act as the agent", AGENT, &[])
            .map_err(|error| error.to_string())?;
        let found = Repo::of(&repo).ok_or("not a repository")?;
        let brought = bring_back(&openshell, &name, &found).map_err(|error| error.to_string())?;
        let again = bring_back(&openshell, &name, &found).map_err(|error| error.to_string())?;
        Ok::<_, String>((String::from_utf8_lossy(&given).into_owned(), brought, again))
    })();
    let _ = openshell.delete(&name);
    let (given, brought, again) = tried.unwrap();

    assert_eq!(
        given, "my notes\n",
        "given the person's files as they were, with nothing left to commit"
    );
    assert_eq!(
        git(&repo, &["rev-parse", "HEAD"]),
        head,
        "their branch is where it was"
    );
    assert_eq!(
        git(&repo, &["status", "--porcelain"]),
        status,
        "their index and files are as they were"
    );
    assert_eq!(brought.branch, format!("playpen/{name}"));
    assert_eq!(
        brought.commits, 2,
        "the agent's commit and one of its uncommitted work"
    );
    let seed = format!("refs/playpen/seed/{name}");
    let diff = git(&repo, &["diff", "--name-status", &seed, &brought.branch]);
    assert_eq!(diff, "D\tLICENSE\nM\tREADME.md\nA\tlogo.bin");
    let readme = git(&repo, &["diff", &seed, &brought.branch, "--", "README.md"]);
    assert!(
        readme.contains("+agent line") && !readme.contains("+mine"),
        "only the agent's: {readme}"
    );
    let logo = git(
        &repo,
        &["cat-file", "-s", &format!("{}:logo.bin", brought.branch)],
    );
    assert_eq!(logo, "3000");
    assert_eq!(again, brought, "nothing new the second time");
    let _ = std::fs::remove_dir_all(&root);
}
