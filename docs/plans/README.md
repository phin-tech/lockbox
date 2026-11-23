# What more of OpenShell Playpen could show

2026-11-23. This compares OpenShell **v0.1.9** with what Playpen does today. The sources are `openshell --help`, with
the hidden `rule` and `workspace` commands included; the docs at https://docs.nvidia.com/openshell/v0.1.9/; the release
notes at https://github.com/NVIDIA/OpenShell/releases; and the third-party write-ups each plan cites.
`docs/research/openshell.md` has more on policies and proposals.

Most tutorials written before 0.1.0 describe `inference.local`, `openshell inference` and managed inference routes.
Those were removed in 0.1.0 ([upgrade guide](https://docs.nvidia.com/openshell/v0.1.9/upgrade/0-1-0)). A model is now
reached through a provider attached to the sandbox, which is what Playpen already does. Nothing here plans for inference
routes.

## What Playpen already does

Playpen already lists, starts, stops, restarts and deletes sandboxes. New Agent… makes them, with uploads, labels,
`--approval-mode manual` and providers. It also opens a session in a sandbox and brings work back. Ports are forwarded
(`forward start/stop/list`). Network activity is read from the audit lines in `logs`. Proposals are approved, rejected
and approved all at once (`rule get/approve/reject`). Services are attached and detached (`sandbox provider` plus
`policy update` with `playpen-service-*` rules). Providers and profiles are added, given a new key (`provider update`)
and renewed for a ChatGPT sign-in (`provider refresh`). Gateways are added, signed in to, removed and selected, and the
MicroVM gateway is set up.

## The plans, in the order to do them

| # | Plan | What it adds | Size |
|---|---|---|---|
| 1 | [Policy](policy.md) | See a sandbox's policy, its revisions, and allow or deny a host by hand | Large |
| 2 | [Proposals](proposals.md) | Clear, history, a reason with Reject, and letting the agent propose | Medium |
| 3 | [Credential trouble](credentials.md) | Say when a provider's key failed and how to fix it | Small |
| 4 | [Suspend, editors and files](lifecycle-and-access.md) | Suspend and resume, Open in VS Code or Cursor, download a file | Medium |
| 5 | [Logs](logs.md) | The whole log, followed live, by level and source | Medium |
| 6 | [Gateway links](gateway-links.md) | A sandbox's service reachable at a URL the gateway gives | Small |
| 7 | [Making options](making-options.md) | CPU, memory, GPU and restart policy in New Agent… | Medium |
| 8 | [Gateway health](gateway-health.md) | Who you are on a gateway, its version and drivers, sign out, Docker check | Small |
| – | [Later](later.md) | Workspaces, profile tools, audit export, the prover, and what's left out | — |

Plans 1–3 are about deciding what a sandbox may reach, which is Playpen's purpose. Plans 4–8 can go in any order.

## Decisions these depend on

Accepted 2026-11-23:

- [ADR 0002](../adr/0002-playpen-edits-a-policy-by-named-rules.md): Playpen changes a policy one named rule at a time,
  never by replacing the whole file.
- [ADR 0003](../adr/0003-openshell-settings-are-set-per-sandbox.md): OpenShell's settings are set on a sandbox, never
  for the whole gateway.
- [ADR 0004](../adr/0004-sandboxes-are-not-made-from-openshell-templates.md): Playpen doesn't make sandboxes from
  OpenShell templates.

The CLI-or-gRPC question is already settled. `crates/core/src/openshell/mod.rs` says Playpen runs the command, because
the command already holds the gateway's mTLS client certificate. The plans keep to that, including where it means
parsing text (`rule history`, `logs`).

## Words to settle first

OpenShell uses three words that Playpen already uses for something else. None of these is in `CONTEXT.md` until it's
agreed:

- **Settings.** Playpen's Settings are what the Settings window keeps. OpenShell's `settings` are four switches on a
  sandbox or gateway. Proposal: don't name them in the app at all. Show each switch where it acts ("Let the agent
  propose rules" under Network requests), and never call it a setting.
- **Service.** Settled. Playpen keeps **Service** for an outside API reached with a provider's key, and OpenShell's
  `service expose` is a **Gateway link** (plan 6). Both are in `CONTEXT.md`.
- **Template.** It sits next to Kit and means something different: a runtime shape (image, CPU, memory, GPU), where a
  Kit is a set of keys. ADR 0004 proposes not adopting templates, so no word is needed.
