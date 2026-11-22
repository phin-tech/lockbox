# Bringing an agent's work back from an OpenShell sandbox

2026-11-21. Research for Playpen. The findings cover **OpenShell v0.1.8**, the version installed here
(`openshell --version` prints `openshell 0.1.8`). The source is tag `v0.1.8` (commit
`f3d976590d3eb1379690a2abaf0f0e26c21d487d`) of https://github.com/NVIDIA/OpenShell. The repository now redirects to
`ai-openshell/openshell`, where its issues live. `openshell.md` covers v0.1.9. Between the two tags, the transfer code
(`crates/openshell-cli/src/ssh.rs`, the supervisor's `ssh.rs`) changed only in how TCP connections are opened, and
`run.rs` changed nothing about git filtering. So what follows holds for both.

Code citations give a path and line numbers at the tag, under `https://github.com/NVIDIA/OpenShell/blob/v0.1.8/`.
Docs citations give the source file at the tag. Its published page is the path without `docs/` and `.mdx` under
https://docs.nvidia.com/openshell/latest/; most of them are `docs/how-it-works/sandboxes/overview.mdx` ("Sandboxes"
below). No sandbox, provider or policy was created or changed. The only things run were `--help` output and a
host-only git experiment in a scratch directory.

## What this means for Playpen

| Question | Answer |
|---|---|
| Does OpenShell have a way to get changes out? | **Only `sandbox download`, a one-shot tar copy.** It applies no `.gitignore` filter and includes `.git`. It overwrites existing files, never deletes anything, and shows no preview. There is no sync, watch or git integration. Maintainers turned down bidirectional host mounts "for security reasons". |
| Is `.git` left out of uploads on purpose? | **It's a side effect.** Filtered uploads send exactly what `git ls-files -co --exclude-standard` lists, and git never lists `.git`. `--no-git-ignore` (on `upload` and `create --upload`) uploads a plain recursive tar, `.git` included. |
| Is `sandbox exec` stdout binary-safe? | **Yes, from source; untested.** Stdout travels as protobuf `bytes`, apart from stderr. The docs say large output is backpressured and that a delivery failure makes `exec` fail. Use `--no-tty --no-login-shell`, or a login profile can write into the stream. Stdin is capped at 4 MiB. |
| How do other tools do it? | **Git refs, mostly.** container-use, Docker Sandboxes' clone mode, Claude Code cloud sessions and Cursor hand back a branch, and Codex cloud hands back a diff applied with `git apply --3way`. File sync appears only where the host directory is mounted in (Docker direct mode, dev containers) or as Coder Desktop's sync, which leaves out `.git`. |
| What should Playpen build? | **Bundles in, bundles out, into a `playpen/<sandbox>` branch.** Seed the sandbox from a `git bundle`. To bring work back, snapshot the sandbox's tree, including uncommitted and untracked files, to a ref without touching the agent's index. Stream an incremental bundle out over `exec` stdout and `git fetch` it into `refs/heads/playpen/<sandbox>`. For Tilt, keep a dedicated git worktree and move it to that branch after each fetch. |

**Biggest unknowns.** None of these has been tested against a live sandbox:

- that a multi-megabyte binary stream over `exec` stdout arrives byte-for-byte;
- that `git` is present in the agent image;
- how long a fetch cycle takes.

The experiments at the end check all three.

---

## 1. What OpenShell provides

### Upload, and why `.git` is left out

- **Help text.** "Inside a Git work tree, `.gitignore` rules are applied by default. Outside a Git work tree,
  uploads proceed unfiltered with a warning. Filtering errors or empty selections stop the upload; use
  `--no-git-ignore` to intentionally upload everything." (`openshell sandbox upload --help`, 0.1.8)
- **How the filter works.** `sandbox_upload_plan` finds the repo root with `git rev-parse --show-toplevel`. It then
  builds the file list from `git ls-files -co --exclude-standard -z`, limited to the uploaded path
  (`crates/openshell-cli/src/run.rs` L5207–5285, the command at L5243; the plan at L5287–5318). `-co` means cached
  plus untracked-but-not-ignored files. Git never lists its own `.git` directory, so `.git` is always left out. The
  code contains no `.git` rule of its own.
- **Uploading `.git` directly.** The pathspec `.git` matches nothing, and an empty selection is an error: "Git
  filtering selected no files … Use --no-git-ignore to upload intentionally without filtering" (`run.rs`
  L5307–5316). That fits the earlier experiment in which nothing was copied, though at this tag the CLI should also
  have printed that error.
- **The stated reason** is build outputs, not `.git`: uploads respect `.gitignore` "so that build artifacts,
  dependency caches, and other ignored files are not transferred" (Sandboxes, "Transfer Files", line 967). I found
  no doc or issue saying `.git` is left out deliberately.
- **Including `.git`.** With `--no-git-ignore`, or with a symlink as the source, the plan is `Regular` (`run.rs`
  L5299–5301). It tars the path recursively with `read_dir` and applies no exclusions
  (`crates/openshell-cli/src/ssh.rs` L1056–1160, `append_upload_dir_contents` at L1140). So
  `openshell sandbox upload --no-git-ignore NAME ./repo/.git /sandbox/repo/` should carry `.git`. That is inferred
  from the code and untested.
- **Merging and symlinks.** An upload merges into an existing directory and "overwrites matching entries without
  deleting unrelated entries". Symlinks, dangling ones included, arrive as symlinks. A cancelled upload leaves
  partial files behind. Total size isn't limited by the 4 MiB stdin budget, because uploads stream with
  backpressure (Sandboxes, "Transfer Files", lines 912–940).
- **`sandbox create --upload LOCAL:DEST`** takes the same rules and `--no-git-ignore` (`crates/openshell-cli/src/main.rs`
  L1475–1490). Every upload is checked before provisioning, and it cannot be combined with a trailing command
  (Sandboxes, lines 946–985).
- **There is no `--sync`.** An old example documented `sandbox create --sync`, but the flag was never implemented
  and the docs were changed to `--upload .` (issue
  [#1339](https://github.com/ai-openshell/openshell/issues/1339)).

### Download

The implementation is `sandbox_sync_down` and its helpers (`ssh.rs` L1690–1957).

- **Mechanism.** It opens an SSH session through the gateway and runs `tar cf - -C <path> .` inside the sandbox
  (L1955). On the host, the Rust `tar` crate 0.4.46 unpacks the stream (`stream_sandbox_tar`, L1797–1845). The
  sandbox image must therefore contain `tar`.
- **No filtering.** Nothing like the upload filter is applied. Everything under the path comes back, `.git`,
  `node_modules` and `.venv` included. A request to apply `.gitignore` to downloads is open
  ([#5514](https://github.com/ai-openshell/openshell/issues/5514)). It warns that a `.git` cloned inside the sandbox
  "overwrites ssh origins on the host".
- **Overwriting.**
  - A directory download extracts straight into `DEST`, creating it if needed. Existing files are overwritten,
    since the `tar` crate defaults to `overwrite: true` (`tar-0.4.46/src/archive.rs` L55–69).
  - Files deleted in the sandbox are never deleted on the host.
  - A single file is staged in a temporary directory and renamed into place, which is atomic on the same
    filesystem. It refuses to replace a directory (`place_downloaded_file`, L1902–1930).
  - A request to preview and confirm what a download will write is open
    ([#4478](https://github.com/ai-openshell/openshell/issues/4478)).
- **Not incremental.** Every call copies the whole tree.
- **Permissions and ownership.** These follow from the `tar` crate's defaults (`archive.rs` L55–69, `entry.rs`
  L805–815):
  - Ownership is not kept, so files belong to the Mac user running the CLI.
  - Mode bits are masked to `0o777`, which keeps executable bits and drops setuid, setgid and sticky.
  - Modification times are kept. Extended attributes are not.
- **Path safety.**
  - Symlinks are recreated as symlinks.
  - Entries containing `..` are skipped (`entry.rs` L386–415).
  - Nothing is written through a symlink that resolves outside `DEST` (`validate_inside_dst`, `entry.rs`
    L924–950).
- **Only from the working directory.** The CLI finds the session's working directory with `pwd -P` and refuses any
  source outside it, including one that escapes through a symlink (`ssh.rs` L1380–1464; Sandboxes, "Transfer
  Files"). The working directory is `/sandbox`, or the image's `WORKDIR` on Docker and Podman
  (`docs/how-it-works/sandboxes/runtimes.mdx` line 371). Playpen has to keep the repo, and any bundle it writes,
  under that directory and out of `/tmp`.
- **Connections and retries.** A download uses three SSH connections: the resolve probe, the type probe and tar
  (`ssh.rs` L1433). The gateway allows 20 concurrent SSH relays per sandbox by default
  (`crates/openshell-core/src/config.rs` L50, L330–331). Transient failures are retried (`retry_sandbox_sync`,
  L1743–1790).

### Exec

- **Binary-safe output.**
  - The server relays SSH channel data into `ExecSandboxStdout { bytes data }` without changing it
    (`proto/openshell.proto` L2042–2049; `crates/openshell-server/src/grpc/sandbox.rs` L4048–4058).
  - The CLI writes those bytes raw to its stdout (`run.rs` L2128–2141). Stderr is a separate stream.
  - The docs say: "Large stdout and stderr streams are delivered before a successful exit. A slow reader
    backpressures the command. If output delivery fails, `exec` returns a failure instead of reporting success with
    incomplete output." (Sandboxes, "Execute a Command", lines 349–351)
  - If the 1 MiB retained-output buffer drops chunks, the sandbox writes "openshell: exec output could not be
    delivered intact" and exits 74 (`crates/openshell-sandbox/src/boundary_server.rs` L4678–4693; buffer at
    `crates/openshell-sandbox/src/main_session.rs` L27).
  - Loss therefore shows up as a failure rather than a silent truncation. Playpen should still check what it
    receives; `git fetch` does that for a bundle.
- **TTY.** Without a TTY, output passes through unchanged. A TTY is used only when both stdin and stdout are
  terminals, so a GUI's pipes don't get one. Pass `--no-tty` anyway, because a PTY would translate line endings.
- **Login shell.** By default `exec` runs `bash -lc`, so `.bash_profile` or `.profile` runs first and can write to
  stdout. `--no-login-shell` prevents that (Sandboxes, "Skip shell startup files", lines 377–388). On 0.1.8, profile
  output corrupts the upload and download helpers' own probes
  ([#5584](https://github.com/ai-openshell/openshell/issues/5584), open). `exec --no-login-shell` avoids it.
  `sandbox download` cannot, because its probes don't set the opt-out.
- **Stdin limits.**
  - The CLI caps piped stdin at 4 MiB, whether sent in one piece or streamed. Above that it fails with "use
    `sandbox upload` for larger input" (`run.rs` L1865–1866, L1978–1980, L2680–2685).
  - The supervisor also limits pending stdin to 4 MiB per channel (Sandboxes, line 321).
  - The command and environment must fit in 1 MiB (`run.rs` L2079–2083).
  - Large data therefore goes in with `upload` and comes out over stdout.
- **Lingering output.** "If a background process keeps stdout or stderr open for more than 30 seconds after the
  command exits, `exec` reports an output delivery failure." (Sandboxes, lines 352–353)

### SSH: a route for real git, rsync or mutagen

- **The config entry.** `openshell sandbox ssh-config NAME` prints a `Host openshell-NAME.WORKSPACE` block for
  `User sandbox`, with `ProxyCommand openshell ssh-proxy …` (`ssh.rs` L2135–2153). The docs offer it for "tools like
  VS Code Remote-SSH" (Sandboxes, "SSH Config", line 879).
- **What the server supports.** The sandbox's SSH server is OpenShell's own (russh), not `sshd`:
  - It accepts `exec` requests and runs them through a shell (`crates/openshell-supervisor-process/src/ssh.rs`
    L726–758, `start_shell` L1304–1350).
  - It serves `sftp` from a built-in helper, so the image needs no `sftp-server` (L899–915).
  - It honours one environment variable, `OPENSHELL_NO_LOGIN_SHELL=1`, which skips the login shell (L935–960).
    `ssh -o SetEnv=OPENSHELL_NO_LOGIN_SHELL=1` should therefore give clean exec channels, though that is inferred
    and untested.
- **What that makes possible.** `git fetch openshell-N.default:/sandbox/repo` (git runs `git-upload-pack` over the
  exec channel), `rsync -e ssh` (which needs `rsync` in the image), and `scp` or `sftp`.
- **The SFTP stall.** SFTP, and therefore modern `scp`, stalls for 30 s after the transfer and then exits 74
  ([#5658](https://github.com/ai-openshell/openshell/issues/5658), open, reported on `main` after 0.1.9). The cause
  it names is a cloned helper output socket. The same clone is in v0.1.8
  (`crates/openshell-sandbox/src/boundary_exec.rs` L92–102), so 0.1.8 probably has the bug too. That is inferred.
- **Mutagen is a poor fit.** Mutagen v0.18.1 installs its agent over SSH with `scp`
  ([`pkg/agent/transport/ssh/transport.go`](https://github.com/mutagen-io/mutagen/blob/a225ae50aee3d7ebb59139203cb84e8a6a3ff4bf/pkg/agent/transport/ssh/transport.go)
  L77–135), so it would hit #5658. It also needs a Linux agent binary in the sandbox and does no review step.

### Mounts and persistence

- **Persistence.** `sandbox stop` / `start` keep "persistent workspace data" (Sandboxes, "Stop and Start", lines
  989–1004).
  - Docker and Podman with a custom `WORKDIR`: the files live in the container's writable layer and are gone once
    the container is removed: "Export files you need to retain before deleting the sandbox." (`runtimes.mdx` line
    373)
  - The Pi tutorial says plainly: "Pi's changes stay inside the sandbox. To copy them back, run `openshell sandbox
    download`" (`docs/tutorials/run-pi-with-openrouter.mdx` lines 203–205).
- **Mounts.**
  - Docker named volumes labelled `openshell.ai/sandbox-attachable=true` can be mounted through
    `--driver-config-json` (`runtimes.mdx` lines 150–167). That needs `allow_driver_config`.
  - Host **bind** mounts require `enable_bind_mounts = true` and turning resource admission off. The docs warn they
    "can bypass workspace isolation and filesystem policy" (lines 169–181).
  - In the MicroVM driver, libkrun may let a compromised guest reach other host files (line 260).
  - These are gateway-operator settings, not something Playpen should switch on for a user's agent.
- **The maintainers' position.** Closing a request for `--mount`: "We wanted to avoid exposing bidirectional sync
  for security reasons. It also limits our ability to deploy this stack to the cloud. If there are improvements to
  the upload/download feature that go through the gateway we can consider those." (drew, 2026-03-23,
  [#500](https://github.com/ai-openshell/openshell/issues/500))

---

## 2. How comparable tools return work

container-use (CU) and Codex (CX) are pinned to commits:

- CU: https://github.com/dagger/container-use/blob/2e43e625e95216b719ec9338f4034fd3a0be2734/
- CX: https://github.com/openai/codex/blob/8653ca9e99b2c805107bde7b7d6f57a634c643d0/

I re-checked the code lines for container-use's `merge` and `apply`, Codex's `git apply --3way`, and Docker's clone
mode. The rest of this section comes from a delegated survey of the cited pages.

| Tool | Mechanism | Source |
|---|---|---|
| Dagger container-use | **Git branches.** It creates a host-side bare "fork" repo with `git init --bare` and adds it to the user's repo as the remote `container-use`. Each environment is branch `<env-id>`, checked out as a `git worktree`. The container's workdir is exported into the worktree with `Wipe: true`, auto-committed, and fetched. `checkout` makes a tracking branch, `merge` is `git merge --no-ff`, and `apply` is `git merge --squash`, leaving the changes staged for review. Auto-commit skips binary files and `node_modules`, `target`, `dist` and `.venv`. | CU `repository/repository.go` L146, L215, L623, L635; `repository/git.go` L149–214, L286–318, L376–399, L715–760; `docs/environment-workflow.mdx` L11, L118, L248 |
| Docker Sandboxes (`sbx`) | **Two modes.** *Direct mode* mounts the project at "the same absolute path as on your host", and changes in either direction are "instant", so errors and build outputs point at paths you can find on the host. *Clone mode* gives the agent a private clone, with the host repo read-only at `/run/sandbox/source`. The host gets a remote `sandbox-<name>`, you `git fetch` it, and "`sbx stop` makes the remote unavailable." Cloud sandboxes have only `sbx cp`. | https://docs.docker.com/ai/sandboxes/architecture/, https://docs.docker.com/ai/sandboxes/workflows/git/, https://docs.docker.com/ai/sandboxes/cloud/local-vs-cloud/ |
| Claude Code | **Two answers.** The *dev container* bind-mounts the repo, so edits "appear in your local repository as you work". The *Bash sandbox* (Seatbelt or bubblewrap) is an access boundary over the real directory, and it denies writes to `.git/hooks`. *Cloud sessions* push a branch to GitHub through a credential-holding proxy that "doesn't limit which branches". The diff view appears after the push, and `--teleport` fetches and checks out that branch. Without GitHub, `--cloud` uploads a git **bundle** of the local repo, and pushing back to other hosts is not possible. | https://code.claude.com/docs/en/devcontainer, https://code.claude.com/docs/en/sandboxing, https://code.claude.com/docs/en/claude-code-on-the-web, https://code.claude.com/docs/en/cloud-environments |
| OpenAI Codex cloud | **A diff.** `codex apply` "Apply the latest diff generated by a Codex cloud chat to your local working tree". It fetches the task's `output_diff` and runs `git apply --3way`, with a `git apply --check` preflight. A PR can also be opened. No reason is given for a diff over a branch. | https://learn.chatgpt.com/docs/developer-commands?surface=cli; CX `codex-rs/chatgpt/src/apply_command.rs` L38–75, `codex-rs/git-utils/src/apply.rs` L40–80 |
| Cursor cloud agents | **A branch push and a PR** to the user's git host, with a "narrower token" inside the VM. Branch naming and checking a branch out locally are not documented. | https://cursor.com/docs/cloud-agent, https://cursor.com/docs/integrations/github |
| GitHub Codespaces | **`git push` from inside**, to a fork when the user lacks write access, plus `gh codespace cp` for files. | https://docs.github.com/en/codespaces/developing-in-a-codespace/using-source-control-in-your-codespace, https://cli.github.com/manual/gh_codespace_cp |
| Daytona | **A file API and in-sandbox git.** `sandbox.fs.download_file(s)` and streaming, plus `sandbox.git` clone, commit and push with credentials. There is no return-to-repo flow. | https://www.daytona.io/docs/en/file-system-operations/, https://www.daytona.io/docs/en/git-operations/ |
| E2B | **A file API.** `files.read`, signed download URLs and `watchDir`. `sandbox.git` is deprecated: "Run git through sandbox.commands.run() instead." | https://docs.e2b.dev/filesystem/download.md, https://docs.e2b.dev/filesystem/watch.md, https://docs.e2b.dev/sandbox/git-integration.md |
| Modal Sandboxes | **A file API, volumes and snapshots.** `copy_to_local` and similar, Volumes committed in the background, and filesystem snapshots as Images. No git helpers. | https://modal.com/docs/guide/sandbox-files, https://modal.com/docs/guide/sandbox-snapshots |
| Coder | **`git push` from the workspace**, with injected credentials. Coder Desktop's file sync is bidirectional but "excludes … `.git/`". An open issue explains that git "was not developed with bi-directional file syncing of the metadata in mind". | https://coder.com/docs/admin/external-auth, https://coder.com/docs/user-guides/desktop/desktop-connect-sync.md, https://github.com/coder/coder-desktop-macos/issues/164 |

**Why git, in these tools' own words:**

- **Review before anything lands:** container-use's `apply`, Docker's fetch-then-diff, and Claude's diff view and PR.
- **Credentials stay out of the sandbox:** Claude's proxy and Cursor's narrower token.
- **Don't sync `.git` in both directions:** Coder.

Only Docker direct mode argues for live file sharing, and its reason is identical paths, not safety. None of these
tools syncs a sandbox's `.git` back over the user's.

---

## 3. Mechanisms Playpen can build with exec, upload and download

The git commands below come from https://git-scm.com/docs/git-bundle, https://git-scm.com/docs/git-commit-tree,
https://git-scm.com/docs/git-format-patch and https://git-scm.com/docs/git-worktree.

### Getting the repo in

- **Bundle (recommended).** On the host, run `git bundle create seed.bundle --all`, or the branches you want.
  Write it to a temporary directory outside any work tree, so the upload is unfiltered (it warns) and the bundle
  isn't dropped by `.gitignore`.
  - Upload it with `sandbox upload`. Size isn't limited.
  - In the sandbox, `git clone seed.bundle /sandbox/<repo>`, then remove the `origin` remote or point it nowhere.
  - The sandbox gets history but none of the host's `.git/config` (remote URLs that may carry tokens), hooks, or
    absolute worktree and alternates paths.
  - Record the seed commit; every bundle coming back is relative to it.
- **Dirty host state.** A bundle carries only refs. To include uncommitted host edits, snapshot them to a commit
  first (as below), or follow the bundle with a git-filtered `sandbox upload` of the working tree, which carries
  modified and untracked-unignored files.
- **Plain `upload --no-git-ignore` of `.git`** is possible (§1). It copies the host's config, hooks and packs
  wholesale, and a linked worktree's `.git` is only a pointer file, so it is the worse choice.
- **Not over exec stdin.** Exec stdin stops at 4 MiB.

### Getting the work out

1. **Snapshot without disturbing the agent.**
   - Inside the sandbox, run `exec --no-tty --no-login-shell`.
   - Copy `.git/index` to a temporary index, set `GIT_INDEX_FILE` to it, and run `git add -A` and
     `git write-tree`.
   - Then run `git commit-tree <tree> -p HEAD -m "playpen snapshot"` and `git update-ref refs/playpen/snapshot
     <commit>`.
   - This captures commits, staged and unstaged edits, deletions, untracked files and binaries, and leaves out what
     `.gitignore` ignores. The agent's index, `HEAD` and branch stay as they were.
   - Comparing the tree hash with the previous one gives a cheap "has anything changed?" check.
2. **Bundle only what's new.** `git bundle create - refs/playpen/snapshot <agent-branch> ^<last-fetched-commit>`,
   written to stdout. This was checked on the host with git 2.50.1:
   - a base commit, then an agent commit, a deletion, a 3000-byte random binary, a staged file, an untracked file
     and an ignored `node_modules/`;
   - the incremental bundle was 3.7 KB, and fetching it gave all four file changes with the binary intact;
   - `node_modules` was left out, and `git status` in the "sandbox" was unchanged.
3. **Carry it out.** Read the bundle from `exec` stdout into a file on the Mac (§1, binary-safe from source). This
   path also escapes #5584. If it proves unreliable, write the bundle under the workspace, such as
   `/sandbox/.playpen/out.bundle`, and use `sandbox download`.
4. **Land it in a branch.** On the host, `git bundle verify out.bundle`, then
   `git fetch out.bundle +refs/playpen/snapshot:refs/heads/playpen/<sandbox>`.
   - Fetching checks the pack's integrity and that the prerequisites exist.
   - It never touches the user's working tree, index or hooks.
   - The user reviews with `git diff main...playpen/<sandbox>`, then merges, squashes or cherry-picks. That is
     container-use's `merge` / `apply` split.
   - Fetching the agent's own branch into `playpen/<sandbox>-commits` keeps its commit history as well.

**Other options, and why not:**

| Mechanism | Binary files | Deletions | Large repos | Why not |
|---|---|---|---|---|
| `git diff --binary` → `git apply --3way` (Codex) | Only with `--binary` | Yes | Fine | Loses commits and authorship. A text diff is fine for a review pane. |
| `git format-patch` → `git am` | Yes; format-patch implies `--binary` | Yes | Fine | Linear commits only. Uncommitted work has to be committed first, and `am` touches the user's working tree. |
| `sandbox download` of the tree | Yes | **No** | Copies everything every time | Overwrites host files. Brings `.git`, `node_modules` and `.venv` (#5514), with no review (#4478). |
| tar over exec stdout | Yes | No, without a manifest | Full copy each time | Overwrite-only, the same as download, and Playpen would be doing git's job. |
| rsync over `ssh-config` | Yes | With `--delete` | Incremental | Needs `rsync` in the image, has no review step, and is untested against OpenShell's SSH server. |
| git over `ssh-config` (`git fetch openshell-N.default:/sandbox/repo`) | Yes | Yes | Incremental, negotiated | A good second phase. It skips bundle files, but is untested and needs `SetEnv=OPENSHELL_NO_LOGIN_SHELL=1` to keep profiles out of the protocol. |
| mutagen | Yes | Yes | Incremental, live | Its agent install uses `scp`, which hits #5658. It needs an agent binary in the image, and syncing `.git` both ways is unsafe (Coder #164). |
| Host bind mount | n/a | n/a | Live | Needs the gateway operator to enable bind mounts and turn off admission. That bypasses filesystem policy, against the maintainers' stance in #500. |

### Continuous sync for Tilt and kind

- **A dedicated worktree.** Keep one per sandbox, outside the user's checkout:
  `git worktree add --detach ~/Library/Application\ Support/Playpen/worktrees/<sandbox> playpen/<sandbox>`.
- **Polling.** Every few seconds:
  1. Run the snapshot step and compare the tree hash; if nothing changed, stop there.
  2. Fetch the incremental bundle.
  3. Run `git -C <worktree> checkout --detach --force playpen/<sandbox>`, or `reset --hard`.
- **What the checkout does.** It rewrites only the files that changed, deletes files the agent deleted, and leaves
  ignored build outputs (`node_modules`, Tilt caches) in place. Tilt's watcher sees small, real changes.
- **Why not the alternatives.** A tar or download overlay never deletes. A bind mount isn't available by default.
  Pointing Tilt at the worktree keeps the user's own checkout untouched.
- **Latency.** Each cycle is two or three gRPC execs. Untested.

---

## 4. Gotchas

- **Profiles.** Always pass `--no-tty --no-login-shell` for machine-readable exec. `sandbox download` and `upload`
  are broken on 0.1.8 whenever a login profile prints to stdout (#5584).
- **Stdin.** It is capped at 4 MiB, so large data goes in with `upload`.
- **Download location.** Only sources inside the working directory (`/sandbox` or `WORKDIR`) can be downloaded.
- **Background processes.** A background process holding stdout open turns a successful exec into a failure after
  30 s. Redirect its output or detach it.
- **Ownership.** Downloaded files belong to the Mac user. Executable bits and mtimes survive; setuid and xattrs
  don't. Symlinks come back as symlinks, possibly pointing anywhere, though nothing is written through one that
  leaves `DEST`.
- **Downloading over a checkout.** It overwrites without asking and never deletes. A downloaded `.git` could carry
  the agent's hooks and config onto the Mac, where git would then run them. Never download into the user's
  checkout.
- **Required tools in the image.** Download needs `tar`, and every git mechanism needs `git`. The default image is
  a minimal `nvcr.io/nvidia/base/ubuntu:24.04` (Sandboxes, "Default Workload Image", line 250). Whether it ships
  `git` is not documented, so Playpen should check for `git` and fail clearly if it's missing.
- **Workspace lifetime.** On Docker and Podman with a custom `WORKDIR`, the workspace dies with the container.
  Fetch before deleting.
- **The SFTP stall.** `scp`, `sftp` and VS Code Remote file transfer probably stall and exit 74 on 0.1.8 (#5658).

## Recommended approach for Playpen

1. **Seed.** On the host, create a git bundle (plus a snapshot commit of the dirty tree, if the user wants it).
   Upload it with `openshell sandbox upload` from a temporary directory, then `git clone` it to `/sandbox/<repo>`
   inside the sandbox. Never upload the host's `.git`.
2. **Return.** Run one `exec --no-tty --no-login-shell` script that snapshots the tree to `refs/playpen/snapshot`
   through a temporary index and writes `git bundle create - … ^<last-fetched>` to stdout. On the Mac, write it to a
   file, `git bundle verify` it, and `git fetch` it into `refs/heads/playpen/<sandbox>`. Keep the last fetched
   commit per sandbox. Use `sandbox download` of a bundle file only as the fallback.
3. **Review.** Show `git diff <base>...playpen/<sandbox>` in the GUI. Offer **merge**, **squash into the working
   tree**, or **check out the branch**, mirroring container-use. Nothing is written to the user's files until they
   choose.
4. **Live use (Tilt).** Keep an opt-in per-sandbox `git worktree`, moved to `playpen/<sandbox>` after each poll that
   finds a new tree hash.
5. **Later.** Try plain git over `openshell sandbox ssh-config` with `SetEnv=OPENSHELL_NO_LOGIN_SHELL=1`. If it
   works, it replaces bundle files with native incremental fetch.

**Experiments before building**, on a disposable sandbox, which this research did not touch:

1. Stream `head -c 50M /dev/urandom | tee /sandbox/x | sha256sum` and `cat /sandbox/x` through
   `exec --no-tty --no-login-shell`, and compare the hashes on the Mac.
2. Check `git --version` in the default and agent images.
3. Run the bundle round trip above end to end, and time it.
4. Run `git fetch` over `ssh -F <(openshell sandbox ssh-config N) -o SetEnv=OPENSHELL_NO_LOGIN_SHELL=1`.
5. Run `sandbox upload --no-git-ignore` of a `.git` directory, to confirm §1.
