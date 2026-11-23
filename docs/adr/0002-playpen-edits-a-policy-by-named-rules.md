---
status: accepted
---

# Playpen edits a policy by named rules, never by replacing it

A sandbox's policy is changed by several hands. Approved proposals add `allow_*` rules, attaching a service adds
`playpen-service-*` rules, a provider contributes `_provider_*` rules, and the agent or the person may change it with
`openshell` directly. Playpen therefore changes a policy only with `policy update`, which adds, changes or removes one
named rule at a time. It never runs `policy set`, which replaces the whole policy. With `set`, Playpen would have to read,
change and write back the whole file, and anything that changed in between, such as an approval landing at that moment,
would be lost.

Playpen changes only the rules a person made or approved: `playpen-rule-*` (added by hand in Playpen) and approved
proposals (`allow_*`). It may remove them, or add a deny path to them, giving the rule's binaries as the policy lists
them. Every other rule is shown read-only: `playpen-service-*` changes only through Attach and Detach, `_provider_*`
belongs to the provider, and anything else was written by someone else.

## Consequences

- Playpen can't write what only `policy set` can: middleware and GraphQL, MCP and JSON-RPC rules
  ([manage policies](https://docs.nvidia.com/openshell/v0.1.9/how-it-works/policies/manage-policies)). It shows them and
  leaves writing them to a policy file and the CLI. Filesystem, Landlock and process settings can't be loosened on a
  running sandbox by any command, so they're read-only anyway.
- A gateway-wide policy replaces the sandbox's, and OpenShell refuses approvals while one is in force. Playpen then shows
  the policy read-only and says why, rather than writing to a policy that isn't the one in effect.
