# Suspend, editors and files

Three small things a person does to a sandbox that Playpen doesn't offer yet.

## Suspend and resume

`sandbox suspend` keeps a Ready sandbox's memory and frees its CPU. `sandbox resume` brings it back where it was, with
the agent mid-thought. Stop, by contrast, keeps only the workspace. Both arrived in v0.1.6
([release](https://github.com/NVIDIA/OpenShell/releases/tag/v0.1.6)), and both need a compute driver and isolation
backend that support them. Playpen already reads the `suspended` phase (`Phase::Suspended`).

- Commands `sandbox.suspend` and `sandbox.resume`, in the Sandbox menu beside Stop. Resume replaces Start for a suspended
  sandbox.
- **The Docker driver can't.** Tried on 0.1.9 on 2026-11-23: `sandbox suspend` on a Ready sandbox was refused with
  "compute driver does not support sandbox suspend" (a failed-precondition error), and the sandbox stayed Ready.
  `gateway info -o json` lists a gateway's compute drivers but no suspend capability. So don't offer Suspend on a gateway
  whose only driver is `docker`, offer it on the others, and if one refuses, show the refusal and remember it for that
  gateway. MicroVM and Kubernetes haven't been tried.

## Open in VS Code or Cursor

`sandbox connect <name> --editor vscode|cursor` keeps the sandbox alive, writes an SSH host entry into an
OpenShell-managed include file rather than `~/.ssh/config` itself, and opens the editor on it
([sandboxes](https://docs.nvidia.com/openshell/v0.1.9/how-it-works/sandboxes/overview)).

- **Open In ▸ VS Code / Cursor** (`sandbox.open_vscode`, `sandbox.open_cursor`), next to the terminals Open already
  offers. Show each only when its app is installed.
- Run it as Playpen runs other `openshell` calls, with `OPENSHELL_GATEWAY` set, off the window's thread.
- **Copy SSH Config** (`sandbox.ssh_config`), from `sandbox ssh-config <name>`, for any other SSH client.
- **What `--editor` touches:** the 0.1.9 binary creates `~/.ssh` if it's missing and adds an `Include "…"` line to
  `~/.ssh/config`, pointing at an `ssh_config` file OpenShell manages. Its errors read "failed to update ~/.ssh/config".
  The exact path of that file wasn't seen, since `--editor` hasn't been run here. Changing `~/.ssh/config` is the person's
  business, so the first use says what it will do and asks.
- `sandbox get` gives `host_key_fingerprint`. Showing it lets a person check the host key their editor is asked to trust.

## Download a file

Bring Back returns git work as a branch. Anything else, such as a build artefact, a log the agent wrote, or a file
outside a repository, has no way out. `sandbox download <name> <path> [dest]` copies a file or folder as a tar. It
includes `.git`, applies no `.gitignore`, and overwrites existing files at the destination
(`docs/research/bringing-work-back.md`).

- **Download…** (`sandbox.download`): a path in the sandbox, defaulting to `/sandbox/`, then a macOS save panel. Always
  download into a new folder, never over existing files, so the overwrite can't do harm.
