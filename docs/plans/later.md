# Later, and left out

What else OpenShell 0.1.9 has, and why it waits.

## Workspaces

Workspaces divide a gateway between teams, and the `workspace` command is hidden. Each has members with roles (Platform
Admin, Workspace Admin, Workspace User), taken from OIDC claims. Every resource call takes `--workspace`, and the
default is `default` ([workspaces](https://docs.nvidia.com/openshell/v0.1.9/how-it-works/workspaces)). A local gateway
without OIDC treats everyone as Platform Admin, so on this Mac there's only ever `default`.

**When:** after a real team gateway with OIDC is in use. Then the gateway's sidebar row could gain a workspace picker,
set as `OPENSHELL_WORKSPACE` the way `OPENSHELL_GATEWAY` is set. A sandbox would then be identified by gateway,
workspace and name, which touches `SandboxKey` and Herdr's naming, so it needs an ADR of its own. Member management stays
in the CLI.

## Provider profile tools

`profile describe` shows a profile's credentials, endpoints and default policy. `profile lint` checks a profile file
without registering it. Playpen imports and exports profiles already. `describe` would help Settings ▸ Providers say
which hosts a provider adds to a policy, which plan 1 also wants for `_provider_*` rules. Pick it up then.

## Audit export and traces

`ocsf_json_enabled` makes the sandbox write its audit events as OCSF JSON, and OTLP sends traces and metrics to a
collector ([observability](https://docs.nvidia.com/openshell/v0.1.9/observability/overview)). Both are for feeding
another system, not for reading in Playpen. If the log viewer (plan 5) wants structured lines, OCSF JSON may be the
cleaner source than parsing text, so check that then. Otherwise leave it.

## The prover

`openshell-prover` checks a policy against a maximum one. Its risk check already runs on every proposal, and Playpen
should show its findings (plan 2's history has the confidence). Running the boundary check by hand is a policy
author's tool. Pick it up if plan 1 grows into whole-policy authoring, which ADR 0002 rules out for now.

## Left out

- **`openshell term`.** It's the TUI that Playpen is.
- **`completions` and `ssh-proxy`.** Shell and SSH plumbing.
- **`--driver-config-json`.** Gateway-admin territory, and off unless the admin enables `allow_driver_config`.
- **`forward service`.** Forwarding over gRPC instead of SSH. Use it if SSH forwarding proves unreliable on some
  gateway; otherwise it's the same feature as Ports.
- **Inference routes.** Removed in 0.1.0.
