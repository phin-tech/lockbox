# Logs: the whole log, followed live

Activity reads only one kind of line from `logs`: the network audit lines (`OCSF NET:*`), the last few hundred, read
when asked. Everything else OpenShell logs is out of reach: the supervisor's warnings, policy loads
(`CONFIG:LOADED`), proposals and approvals, why a sandbox won't start, and the gateway's own lines about the sandbox.
OpenShell's TUI has a log screen with source filtering and line copying, and it's what the docs send a person to first
when something's wrong ([sandboxes, "Inspect Logs and
Activity"](https://docs.nvidia.com/openshell/v0.1.9/how-it-works/sandboxes/overview)).

## What the person sees

**Show Log** (`sandbox.log`, ⌥⌘L or similar) opens a log for the chosen sandbox, as a window of its own, so it can sit
beside the main window:

- Each line shows its time, source (`sandbox` or `gateway`), level and text. Audit lines are shown as Activity shows
  them, as "node reached github.com:443" or "denied".
- Filters: source (`--source`), minimum level (`--level`), and how far back (`--since`, or the last N lines).
- **Follow**, on by default, streams new lines (`logs --tail`) and scrolls with them unless the person has scrolled up.
- A search field, which filters what's loaded. Copying works on a selection.

## How it's built

- `logs` has no JSON output in 0.1.9, so this parses the same text `activity.rs` does. Move `activity.rs`'s line
  parser to a general log-line parser, from which Activity takes the audit lines.
- Following is a child `openshell logs <name> --tail` process, read line by line off the window's thread and stopped when
  the window closes or the sandbox changes. It's the one long-running `openshell` call Playpen would make, so make sure
  quitting, closing the window and choosing another gateway all stop it.
- Keep a fixed number of lines (say 10,000) and drop the oldest.
- A new window kind hands the Commands it doesn't run to `commands::run`, as Settings does.

## Open

- Whether to show the gateway's own log (`--source gateway`) by default. It's noisier, but it's where approvals show.
- The App menu's existing `app.logs` opens Playpen's own log folder. The two need names a person won't confuse:
  "Show Sandbox Log" versus "Show Playpen's Logs".
