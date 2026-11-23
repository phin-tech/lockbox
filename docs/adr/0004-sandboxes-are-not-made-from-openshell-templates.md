---
status: accepted
---

# Playpen doesn't make sandboxes from OpenShell templates

OpenShell's sandbox templates (`sandbox template create`) save a runtime shape on the gateway: image, CPU, memory, GPU,
variables and driver configuration. `sandbox create --template` can't be combined with `--from`, `--cpu`, `--memory`,
`--gpu` or `--env` ([templates](https://docs.nvidia.com/openshell/v0.1.9/how-it-works/sandboxes/templates)). Playpen
names the image on every create, through the gateway's image source (ADR 0001), because the image depends on the agent,
its version and the gateway's platform. So it makes sandboxes with inline flags, and remembers the resources a person
chose for an agent on a gateway in its own settings, beside the image source.

## Considered options

- **A template per agent and gateway, made by Playpen.** This would make the image a template field, so every agent
  update means rewriting templates on the gateway, and templates are workspace-admin objects on a shared gateway.
- **Templates for resources only (no image).** This doesn't work, because `--from` can't be given alongside
  `--template`.
- **Making from a template someone else wrote.** A team gateway's admin might want this, but the template then decides
  the image, and Playpen's agent image and policy may not match it. Revisit if a team gateway asks for it.

## Consequences

A sandbox made from a template outside Playpen still shows and works. `created_from_workload_template` is in `sandbox
get`, and Playpen can show it. Kits stay what they are, sets of keys, and don't become templates.
