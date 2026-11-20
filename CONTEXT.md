# Lockbox

Run your agents in OpenShell sandboxes, and decide what they may reach

The words this project uses. Code, messages and docs use these terms; each entry lists the words to avoid.

## Language

**Command**: something the app does, by name (`file.new`), which a menu item, a key or the palette runs.
_Avoid_: action, which is GPUI's word for the message that carries one

**Settings**: what a person has chosen in the Settings window, kept between runs.
_Avoid_: preferences, config

**Sandbox**: one of OpenShell's isolated environments, where one agent runs, kept by OpenShell's gateway under a
name. Lockbox labels the ones it makes with their agent.
_Avoid_: container, VM, box

**Session**: an agent, or a shell, running in a sandbox in a terminal. A sandbox outlives its sessions. herdr-core
keeps a session when it's installed, so closing the window showing it only detaches; otherwise the window is the
session. The Herdr app, Ghostty or Terminal shows it.
_Avoid_: tab, window, run

**Agent**: a coding agent Lockbox knows how to run in a sandbox (Claude Code), with the image it runs in and the policy
that lets it reach its model.
_Avoid_: client, assistant

**Provider**: OpenShell's name for a credential it keeps, of a type (`claude-code-oauth`) saying what it's for. Its key
never enters the sandbox; OpenShell adds it to the agent's requests to its model.
_Avoid_: key, secret, account

**Policy**: what a sandbox may reach, by host, port and the program connecting. Everything it doesn't allow is denied.
_Avoid_: rules, firewall

**Proposal**: a rule OpenShell drafts from a connection a policy denied, waiting for the person to allow or deny it.
_Avoid_: request, suggestion
