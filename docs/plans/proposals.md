# Proposals: the rest of the inbox, and letting the agent propose

Network requests already lists each sandbox's proposals, and has Approve, Reject and Approve All. Three things are
missing.

## Clear, history and a reason

- **Clear** (`rule clear <name>`) drops every pending proposal. It's for an agent that has spammed the inbox. Ask first,
  since the agent waiting on them is told nothing.
- **History** (`rule history <name>`) is every proposal, approval and rejection, with the time and, for a proposal,
  OpenShell's confidence ("proposed (confidence: 65%)"). The requests view shows only the last few decided, from `rule
  get`. History goes back further and shows when each was proposed. Approvals appear here and in the gateway's log, but
  not in `openshell logs`, which
  [one write-up](https://sorami.com.au/research/nvidia-openshell-agent-sandbox-test/) found confusing (OpenShell
  v0.1.2). Show it under the requests, newest first. `rule history` has no JSON output, so parse its text as
  `activity.rs` parses `logs`, and pin that with an `insta` snapshot of real 0.1.9 output.
- **A reason with Reject.** `rule reject --reason "…"` sends the reason back to an agent that proposed through the
  advisor, so it can try something else
  ([advisor](https://docs.nvidia.com/openshell/v0.1.9/how-it-works/policies/advisor)). Today `reject_request` sends none.
  Add an optional reason field, which stays optional, because a draft from a blocked connection has no agent waiting on
  it.

## The risk check's findings

`rule get` prints each proposal's risk-check result, as `Prover: prover: no new findings` or its findings. Playpen's
parser doesn't read that line. So a flagged proposal looks like any other, and Approve All (`approve_all_requests`)
approves every pending one, flagged or not. `rule approve-all` leaves flagged ones out unless given
`--include-security-flagged`. Read the line into `NetworkRequest` and show any findings on the row. Decided: Approve All
leaves flagged proposals out and says how many it left, and each of those needs its own Approve. Snapshot a flagged proposal's real
output first, since none has been seen here.

## Letting the agent propose

When OpenShell's advisor is on, the agent can read its policy and recent denials, and propose a rule with a reason,
through `http://policy.local` in the sandbox. It's off by default (`agent_policy_proposals_enabled`). Even with it off,
blocked connections become drafts, which is what Playpen shows now. Turning it on gives better proposals: a whole REST
path instead of one host and port, plus the agent's reason.

- A switch under Network requests, **Let the agent propose rules**, runs `settings set <name> --key
  agent_policy_proposals_enabled --value true`. It's set on the sandbox, never the gateway
  ([ADR 0003](../adr/0003-openshell-settings-are-set-per-sandbox.md)).
- Read with `settings get <name> --json`, which gives each key's value and `scope` (`sandbox`, `global` or `unset`).
  When the value comes from the gateway, show the switch as set there and don't let it be changed here.
- New Agent… has a checkbox, **Let the agent propose rules**, off by default. When ticked, the sandbox is made and the
  setting is set on it straight after, before the agent starts asking. Check whether `sandbox create` can take the
  setting itself, so there's no moment without it.

## Not offered: automatic approval

`proposal_approval_mode = auto` approves every proposal the risk check finds nothing wrong with, and that includes any new
public host. In [one test](https://sorami.com.au/research/nvidia-openshell-agent-sandbox-test/) (OpenShell v0.1.2) it
approved 12 of 12. Playpen exists so that a person decides what a sandbox reaches. It already makes every sandbox with
`--approval-mode manual`, and this plan keeps that. If a gateway sets `auto` globally, the requests section says so,
since that overrides the sandbox.

## How it's built

- Core, in `requests.rs`: `clear_requests`, `request_history` (text parser plus snapshot), and `reject_request(name, id,
  reason: Option<&str>)`. In a new `openshell/switches.rs`: `switches(name)` and `set_switch(name, Switch)`, where `Switch` is
  typed to the four keys 0.1.9 registers, each with its own value: two booleans, `manual`/`auto`, and a schema version.
  A key it doesn't know is shown and not edited.
- Commands: `sandbox.clear_requests`, `sandbox.request_history`. While at it, Approve All, which is a button today,
  should also be a Command (`sandbox.approve_all`), as AGENTS.md asks.
