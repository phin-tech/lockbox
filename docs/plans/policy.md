# Policy: see it, and allow or deny by hand

Today a sandbox's policy changes in two ways: a proposal is approved, or a service is attached. A person can't see what
the policy allows, can't see when it changed, and can't add a host before the agent trips over it. The docs teach exactly
that loop: start from default deny, watch the denials, then allow a host, with a read-only L7 rule where possible
([first network policy](https://docs.nvidia.com/openshell/v0.1.9/tutorials/first-network-policy),
[GitHub push access](https://docs.nvidia.com/openshell/v0.1.9/tutorials/github-push-access)). OpenShell's own TUI shows
the policy but can't edit it.

## What the person sees

A **Policy** section for the chosen sandbox, beside Network requests and Activity:

- **Each rule,** by name, with its endpoints (host, port or ports, protocol, `access`, and `enforcement`, where `audit`
  is shown as "watching only") and the programs it covers. Rules are grouped by origin: Playpen's own
  (`playpen-service-*`, `playpen-rule-*`), approved proposals (`allow_*`), a provider's (`_provider_*`, read-only), and
  any others.
- **Where it comes from:** `policy_source` is `sandbox` or `global`. While a gateway-wide policy is in force, the section
  says so, and editing and approving are turned off, because OpenShell refuses both then
  ([advisor](https://docs.nvidia.com/openshell/v0.1.9/how-it-works/policies/advisor)).
- **Revisions,** from `policy list -o json`: version, when, status (`Loaded`, `Superseded` or `Failed`, with a
  shortened error, per [manage policies](https://docs.nvidia.com/openshell/v0.1.9/how-it-works/policies/manage-policies))
  and any error. A failed load matters, because by default the sandbox blocks traffic then
  (`fail_closed`).
- **The filesystem part,** read-only, since it can't change while the sandbox runs.

## What the person does

- **Allow a Host…** (`sandbox.allow`): host, port, optionally the protocol and read-only or read-write access, and the
  programs, defaulting to the agent's. This becomes `policy update <name> --add-endpoint host:port[:access[:protocol]]
  --binary … --rule-name playpen-rule-<host>-<port> --wait`.
- **Deny a Path…** on a `playpen-rule-*` rule or an approved proposal: `--add-deny host:port[,port…]:METHOD:glob`
  with `--rule-name` and the rule's complete `--binary` list, read from the policy. This is for "GitHub, but never
  `POST /repos/*/*/git/refs`".
- **Remove** a `playpen-rule-*` rule or an approved proposal: `--remove-rule <name> --wait`. Other rules are shown
  without these buttons ([ADR 0002](../adr/0002-playpen-edits-a-policy-by-named-rules.md)).
- **Before applying,** run the same arguments with `--dry-run` and show what changes. The CLI prints "Dry run preview",
  then the whole merged policy as YAML, provider rules included. The view diffs that against the current policy. This
  was checked against 0.1.9 here.

Each edit is one named rule, following [ADR 0002](../adr/0002-playpen-edits-a-policy-by-named-rules.md). Playpen never
runs `policy set`.

## How it's built

- Core: `OpenShell::policy(name)` from `policy get <name> --full -o json`. The JSON has `policy.network_policies`,
  `policy.filesystem_policy`, `policy_source`, `version` and `status`, checked against 0.1.9 on this Mac. Add
  `policy_revisions(name)`, `allow(name, &Rule)`, `deny(…)` and `remove_rule(…)`, and snapshot-test the arguments as
  `services.rs` does.
- App: a new `view/policy.rs`. It reads the policy when the sandbox is chosen and after any change, including an
  approval. It doesn't poll.
- Commands: `sandbox.policy` (show the section), `sandbox.allow` (Allow a Host…). Both go in the Sandbox menu.

## Open

- Middleware and GraphQL, MCP and JSON-RPC rules need `policy set`, so per ADR 0002 Playpen shows them but can't
  write them.
- Whether `--binary /**` counts as "any program" isn't documented. `services.rs` already uses it, so check how a revision
  shows it.
