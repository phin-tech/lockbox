---
status: accepted
---

# OpenShell's settings are set on a sandbox, never for the gateway

OpenShell has four settings keys in 0.1.9: `agent_policy_proposals_enabled`, `proposal_approval_mode`,
`ocsf_json_enabled` and `ocsf_schema_version`. Each can be set on a sandbox or for the whole gateway, and the
gateway's value wins wherever it's set. Playpen sets them only on a sandbox. A gateway may be shared, and a gateway-wide
value changes every sandbox on it, including ones Playpen didn't make and people it doesn't know about. While it's set, it
also takes the choice away from each sandbox, since a sandbox value can't override it; only `settings delete --global`
gives the choice back.

## Consequences

- When a key's value comes from the gateway (`settings get` reports `scope: global`), Playpen shows it as set by the
  gateway and doesn't offer to change it.
- Playpen never offers `proposal_approval_mode = auto` at either scope. It makes sandboxes with `--approval-mode manual`
  so that a person decides what each sandbox reaches. A gateway that sets `auto` is shown as doing so.
- Changing a gateway's settings stays a job for its administrator and the CLI (`settings set --global`).
