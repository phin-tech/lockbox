# Playpen

Run your agents in OpenShell sandboxes, and decide what they may reach

The words this project uses. Code, messages and docs use these terms; each entry lists the words to avoid.

## Language

**Command**: something the app does, by name (`file.new`), which a menu item, a key or the palette runs.
_Avoid_: action, which is GPUI's word for the message that carries one

**Settings**: what a person has chosen in the Settings window, kept between runs.
_Avoid_: preferences, config

**Sandbox**: one of OpenShell's isolated environments, where one agent runs, kept by OpenShell's gateway under a
name. Playpen labels the ones it makes with their agent.
_Avoid_: container, VM, box

**Session**: an agent, or a shell, running in a sandbox in a terminal. A sandbox outlives its sessions. herdr-core
keeps a session when it's installed, so closing the window showing it only detaches; otherwise the window is the
session. The Herdr app, Ghostty or Terminal shows it.
_Avoid_: tab, window, run

**Agent**: a coding agent Playpen knows how to run in a sandbox (Claude Code, Codex, Pi, oh-my-pi, PiG), with the image
it runs in and the policy that lets it reach its model. Pi and its kin reach whichever service their key is for.
_Avoid_: client, assistant

**Provider**: OpenShell's name for a credential it keeps, of a type (`claude-code-oauth`) saying what it's for. Its key
never enters the sandbox; OpenShell adds it to the agent's requests to its model.
_Avoid_: key, secret, account

**Policy**: what a sandbox may reach, by host, port and the program connecting. Everything it doesn't allow is denied.
_Avoid_: rules, firewall

**Proposal**: a rule OpenShell drafts from a connection a policy denied, waiting for the person to allow or deny it.
_Avoid_: request, suggestion

**Gateway**: an OpenShell gateway, which keeps sandboxes and runs them on a runtime of its own: Docker on this Mac, or
Kubernetes in a cluster. Playpen can use several, and a sandbox is on the one that made it. Every `openshell` call names
one.
_Avoid_: place, runtime, host, cluster

**Cloud**: what a Gateway extension is for, such as AWS or Google Cloud, offering several kinds of Gateway on it (`ec2`,
`eks`). Its Capabilities are named by it, and its credentials and Grant belong to it.
_Avoid_: provider, which is OpenShell's word for a credential

**Gateway extension**: a folder with a Manifest and JavaScript that adds a Gateway, run in a sandbox and reaching only what
its Manifest asks for and the user has granted.
_Avoid_: plugin, driver

**Manifest**: `gateway.toml` in a Gateway extension's folder, naming it and listing the Capabilities it asks for. Asking is
not having: the file can be changed by an agent, so it can only ask.
_Avoid_: config, permissions file

**Capability**: one thing the host can do for a Gateway extension, typed and named (`gateway.add`). The host does the
work and holds any credential. Only Playpen adds one, in a release.
_Avoid_: permission, scope

**Grant**: the user's permission for a Gateway extension to use a Capability, with its limits, kept in `grants.toml`
beside the Settings, outside the extension's folder. A changed Manifest or code asks again. The default is none.
_Avoid_: approval, allowlist
