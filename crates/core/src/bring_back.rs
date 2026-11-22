//! Bringing an agent's work back: a folder that's a git repository goes into a sandbox with its history, and what the
//! agent does there comes back as a branch, `playpen/<sandbox>`, in the repository on the Mac. Nothing is written to
//! the person's own branches, index or files; merging is theirs to do.
//!
//! OpenShell can't mount a folder or copy `.git` in (its upload sends what `git ls-files` lists), so history travels
//! as git bundles: one in, made from a snapshot of the folder as it is, uncommitted edits and all, so the agent's
//! changes are measured from exactly what it was given; and one out, read from `openshell sandbox exec`'s output,
//! holding only what's new since then. Only history reachable from that snapshot goes in, never other branches.
//! Submodules and Git LFS objects don't travel in a bundle.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::openshell::{OpenShell, OpenShellError};

/// Where the folders are in a sandbox.
const SANDBOX_HOME: &str = "/sandbox";

/// Why work couldn't be brought back, or a folder seeded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BringBackError {
    #[error("git on this Mac failed to {0}: {1}")]
    Git(String, String),
    #[error(transparent)]
    OpenShell(#[from] OpenShellError),
    #[error(
        "“{0}” was copied into the sandbox before Playpen kept its history, so there's nothing to compare its work with."
    )]
    NotSeeded(String),
    #[error(
        "The branch {0} is checked out, and git won't change a branch that's checked out. Switch away from it, then bring the work back again."
    )]
    CheckedOut(String),
}

/// A git repository on the Mac whose top is the folder given, as a linked worktree's can be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub root: PathBuf,
}

/// What came back from a sandbox for one repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Brought {
    pub repo: PathBuf,
    pub branch: String,
    /// Commits on the branch since the seed, counting the one Playpen makes of uncommitted work.
    pub commits: usize,
    /// `git diff --shortstat` from the seed: "3 files changed, 5 insertions(+), 2 deletions(-)", or empty.
    pub changes: String,
}

/// The ref in the Mac's repository holding what the sandbox `sandbox` was given.
fn seed_ref(sandbox: &str) -> String {
    format!("refs/playpen/seed/{sandbox}")
}

/// The branch the sandbox `sandbox`'s work comes back to.
pub fn branch(sandbox: &str) -> String {
    format!("playpen/{sandbox}")
}

/// Runs git in `dir`, giving what it printed, trimmed.
fn git(dir: &Path, doing: &str, args: &[&str]) -> Result<String, BringBackError> {
    git_with(dir, doing, args, &[])
}

fn git_with(
    dir: &Path,
    doing: &str,
    args: &[&str],
    env: &[(&str, &Path)],
) -> Result<String, BringBackError> {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir).args(args).stdin(Stdio::null());
    for (name, value) in env {
        command.env(name, value);
    }
    let output = command
        .output()
        .map_err(|error| BringBackError::Git(doing.to_string(), error.to_string()))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(BringBackError::Git(
            doing.to_string(),
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ))
    }
}

impl Repo {
    /// The repository `folder` is the top of, or none for a folder that isn't one or is only inside one.
    pub fn of(folder: &Path) -> Option<Repo> {
        let top = git(
            folder,
            "find the repository",
            &["rev-parse", "--show-toplevel"],
        )
        .ok()?;
        let same = std::fs::canonicalize(&top).ok()? == std::fs::canonicalize(folder).ok()?;
        same.then(|| Repo {
            root: folder.to_path_buf(),
        })
    }

    /// The folder's name, which is its name in the sandbox too.
    fn name(&self) -> String {
        self.root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// A commit of the folder as it stands, uncommitted and untracked files too, made through an index of its own so
    /// the person's index and `HEAD` stay as they are. It's `HEAD` when nothing's changed.
    fn snapshot(&self) -> Result<String, BringBackError> {
        let doing = "snapshot the folder";
        let head = git(&self.root, doing, &["rev-parse", "--verify", "-q", "HEAD"]).ok();
        let index = git(
            &self.root,
            doing,
            &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        )?;
        let temporary = std::env::temp_dir().join(format!(
            "playpen-index-{}-{}",
            std::process::id(),
            self.name()
        ));
        let _ = std::fs::copy(&index, &temporary);
        let env = [("GIT_INDEX_FILE", temporary.as_path())];
        let tree = git_with(&self.root, doing, &["add", "-A"], &env)
            .and_then(|_| git_with(&self.root, doing, &["write-tree"], &env));
        let _ = std::fs::remove_file(&temporary);
        let tree = tree?;
        if let Some(head) = &head
            && git(
                &self.root,
                doing,
                &["rev-parse", &format!("{head}^{{tree}}")],
            )? == tree
        {
            return Ok(head.clone());
        }
        let mut args = vec![
            "-c",
            "user.name=Playpen",
            "-c",
            "user.email=playpen@localhost",
            "commit-tree",
            &tree,
            "-m",
            "Playpen: the folder as it was given to the sandbox",
        ];
        if let Some(head) = &head {
            args.extend(["-p", head]);
        }
        git(&self.root, doing, &args)
    }

    /// The branch checked out, to give the sandbox's copy the same name.
    fn branch_name(&self) -> String {
        git(
            &self.root,
            "read the branch",
            &["symbolic-ref", "--short", "-q", "HEAD"],
        )
        .ok()
        .filter(|branch| !branch.is_empty())
        .unwrap_or_else(|| "work".to_string())
    }

    /// Whether the sandbox `sandbox` was given this repository's history.
    pub fn is_seeded(&self, sandbox: &str) -> bool {
        git(
            &self.root,
            "look for the seed",
            &["rev-parse", "--verify", "-q", &seed_ref(sandbox)],
        )
        .is_ok()
    }

    /// Forgets what the sandbox `sandbox` was given, once it's deleted. Its branch stays, since it's the work.
    pub fn forget(&self, sandbox: &str) {
        let _ = git(
            &self.root,
            "forget the seed",
            &["update-ref", "-d", &seed_ref(sandbox)],
        );
    }
}

/// Copies `repo` into the sandbox `name` with its history: a snapshot of the folder, bundled, uploaded, and checked
/// out at `/sandbox/<folder>` on a branch named as the Mac's is.
pub fn seed(openshell: &OpenShell, name: &str, repo: &Repo) -> Result<(), BringBackError> {
    let snapshot = repo.snapshot()?;
    let reference = seed_ref(name);
    git(
        &repo.root,
        "keep the seed",
        &["update-ref", &reference, &snapshot],
    )?;
    // Outside any repository, so upload sends it as it is, and named for the folder, which upload keeps.
    let folder = std::env::temp_dir()
        .join(format!("playpen-seed-{}-{name}", std::process::id()))
        .join(format!("seed-{}", repo.name()));
    std::fs::create_dir_all(&folder).map_err(|error| {
        BringBackError::Git("make a folder for the seed".to_string(), error.to_string())
    })?;
    let bundle = folder.join("seed.bundle");
    let bundled = git(
        &repo.root,
        "bundle the folder",
        &[
            "bundle",
            "create",
            "-q",
            &bundle.to_string_lossy(),
            &reference,
        ],
    );
    let uploaded = bundled.and_then(|_| {
        openshell
            .upload(name, &folder, &format!("{SANDBOX_HOME}/.playpen"))
            .map_err(BringBackError::from)
    });
    let _ = std::fs::remove_dir_all(folder.parent().unwrap_or(&folder));
    uploaded?;
    let script = "set -e
dir=\"$1\"; bundle=\"$2\"; branch=\"$3\"
git init -q \"$dir\"
cd \"$dir\"
git fetch -q --no-tags \"$bundle\" \"refs/playpen/seed/*:refs/playpen/seed/*\"
git checkout -q -B \"$branch\" \"$(git for-each-ref --format='%(refname)' refs/playpen/seed | head -1)\"
rm -f \"$bundle\"
";
    let dir = format!("{SANDBOX_HOME}/{}", repo.name());
    let bundle = format!("{SANDBOX_HOME}/.playpen/seed-{}/seed.bundle", repo.name());
    openshell.exec_bytes(
        name,
        &format!("give “{name}” {}'s history", repo.name()),
        script,
        &[&dir, &bundle, &repo.branch_name()],
    )?;
    Ok(())
}

/// What runs in the sandbox to bring work back: a commit of the folder as it stands, made through an index of its
/// own so the agent's stays as it is, then a bundle of everything since the seed on standard output. It exits 3,
/// printing nothing, when there's nothing since the seed.
const SNAPSHOT: &str = "set -e
cd \"$1\"; seed=\"$2\"
index=$(mktemp)
cp \"$(git rev-parse --git-path index)\" \"$index\" 2>/dev/null || true
tree=$(GIT_INDEX_FILE=\"$index\" sh -c 'git add -A >/dev/null 2>&1 && git write-tree')
rm -f \"$index\"
if [ \"$tree\" = \"$(git rev-parse 'HEAD^{tree}')\" ]; then
  commit=$(git rev-parse HEAD)
else
  commit=$(echo 'Playpen: work not yet committed in the sandbox' | git -c user.name=Playpen -c user.email=playpen@sandbox commit-tree \"$tree\" -p HEAD)
fi
git update-ref refs/playpen/snapshot \"$commit\"
[ \"$commit\" = \"$seed\" ] && exit 3
git bundle create - refs/playpen/snapshot \"^$seed\" 2>/dev/null
";

/// Brings the work in the sandbox `name`'s copy of `repo` back to the branch `playpen/<name>` in it, and says how
/// much there is. Nothing else in the repository changes.
pub fn bring_back(
    openshell: &OpenShell,
    name: &str,
    repo: &Repo,
) -> Result<Brought, BringBackError> {
    let reference = seed_ref(name);
    if !repo.is_seeded(name) {
        return Err(BringBackError::NotSeeded(repo.name()));
    }
    let seed = git(&repo.root, "read the seed", &["rev-parse", &reference])?;
    let branch = branch(name);
    let dir = format!("{SANDBOX_HOME}/{}", repo.name());
    let doing = format!("bring back {}'s work from “{name}”", repo.name());
    match openshell.exec_bytes(name, &doing, SNAPSHOT, &[&dir, &seed]) {
        Ok(bundle) => {
            let file = std::env::temp_dir()
                .join(format!("playpen-back-{}-{name}.bundle", std::process::id()));
            std::fs::write(&file, &bundle).map_err(|error| {
                BringBackError::Git("keep the bundle".to_string(), error.to_string())
            })?;
            let path = file.to_string_lossy().into_owned();
            let fetched = git(
                &repo.root,
                "check the bundle",
                &["bundle", "verify", "-q", &path],
            )
            .and_then(|_| {
                git(
                    &repo.root,
                    "fetch the work",
                    &[
                        "fetch",
                        "-q",
                        "--no-tags",
                        &path,
                        &format!("+refs/playpen/snapshot:refs/heads/{branch}"),
                    ],
                )
            });
            let _ = std::fs::remove_file(&file);
            if let Err(BringBackError::Git(_, said)) = &fetched
                && said.contains("checked out")
            {
                return Err(BringBackError::CheckedOut(branch));
            }
            fetched?;
        }
        // Nothing since the seed: the branch is the seed.
        Err(OpenShellError::Exit(3)) => {
            git(
                &repo.root,
                "make the branch",
                &["update-ref", &format!("refs/heads/{branch}"), &seed],
            )?;
        }
        Err(error) => return Err(error.into()),
    }
    let range = format!("{seed}..{branch}");
    let commits = git(
        &repo.root,
        "count the commits",
        &["rev-list", "--count", &range],
    )?
    .parse()
    .unwrap_or(0);
    let changes = git(
        &repo.root,
        "count the changes",
        &["diff", "--shortstat", &seed, &branch],
    )?;
    Ok(Brought {
        repo: repo.root.clone(),
        branch,
        commits,
        changes,
    })
}
