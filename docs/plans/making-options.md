# Making options: resources and restarts

Plain variables (`--env`) aren't here, because Playpen already passes them, from Kits' variables and the agent's own.

New Agent… makes every sandbox the same shape: the gateway's default CPU and memory, no GPU, and no restart. `sandbox
create` takes more, and the most useful of it fits behind a disclosure in the form, **More Options**.

| Option | Flag | Notes |
|---|---|---|
| CPU | `--cpu` | `500m`, `1`, `2.5`. Empty means the gateway's default. |
| Memory | `--memory` | `512Mi`, `4Gi`. Empty means the default. |
| GPU | `--gpu [N]` | Only offered when the gateway can give one. Docker and Podman need CDI, Kubernetes `nvidia.com/gpu`, and a MicroVM takes at most one ([sandboxes](https://docs.nvidia.com/openshell/v0.1.9/how-it-works/sandboxes/overview)). Never on this Mac's Docker. |
| Restart | `--restart-policy` | never (today's), on failure, always. "Always" suits an agent server, such as Codex App Server. |
| Gateway link | `--expose PORT` | See [gateway links](gateway-links.md). |

## Remembered per agent

Whatever was chosen is remembered for that agent on that gateway, in the `[gateways.<name>]` section beside its image
source, so Codex on the cluster always gets 4 CPUs without asking. That's a Playpen setting, not an OpenShell template
([ADR 0004](../adr/0004-sandboxes-are-not-made-from-openshell-templates.md)).

## How it's built

- `Plan::create_args` gains the fields. Snapshot-test the arguments, as the existing `a_sandbox_is_made_detached_…` test
  does.
- Validate CPU and memory as OpenShell would, so a typo is caught in the form, not as an error from `create`.
- Sandbox details show what a sandbox was made with (`restart_policy` and `restart_count` are in `sandbox get`), so a
  sandbox that's restarted 40 times is noticed.

## Open

- Whether a gateway can be asked if it has GPUs, for example from `gateway info`'s drivers. Until it can, show GPU only
  for gateways the person marks as having them.
