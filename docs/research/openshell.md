# OpenShell, for Patchbay and Playpen

2026-11-19. These findings cover **OpenShell v0.1.9**, the current stable release, published 2026-11-17. The sources
are tag `v0.1.9` (commit `ce58cf8e9b953281352d98a637e41616295b1318`) of https://github.com/NVIDIA/OpenShell and the
docs at https://docs.nvidia.com/openshell/latest/, which are labelled "Latest (v0.1.9)". `main` has already moved on:
at commit `7c8e018` its `docs/` folder differs from the tag in 16 files. Nothing here was installed or run.

Most docs pages were read from the `docs/` folder at the tag; that folder is the source of the published site. Each
docs citation gives the published URL, which is the source path without `docs/` and `.mdx` under
`https://docs.nvidia.com/openshell/latest/`. Code citations give a path and line numbers at the tag, under
`https://github.com/NVIDIA/OpenShell/blob/v0.1.9/`.

## What this means for the integration

| Part of the idea | Verdict | Why |
|---|---|---|
| A sandbox's agent reaches Patchbay's gateway on the Mac | **Feasible with one change, unproven on Docker Desktop.** | The sandbox has no network of its own. A trusted supervisor makes every connection on its behalf. The supervisor never connects to a loopback address that the agent names directly. It does connect to a reserved name, `host.openshell.internal`, which the compute driver pins to the host. The MicroVM driver's supervisor is a macOS process, and the code pins that name to `127.0.0.1`, so with that driver Patchbay can stay on `127.0.0.1:7357`. With the Docker driver, the default on a Mac, the name is also pinned to `127.0.0.1`, but that address is inside Docker Desktop's VM, and whether it reaches macOS depends on Docker Desktop. That last step is an inference (see Q1). Either way, the MCP config inside the sandbox must say `http://host.openshell.internal:7357/…`, not `127.0.0.1`, so Patchbay needs a client entry of its own for sandboxes. |
| One allow rule for the gateway, every MCP server host denied | **Feasible.** | Network policy denies by default, so the MCP servers' hosts need no deny rule. They are denied because nothing allows them. One rule under `network_policies` naming `host.openshell.internal`, port `7357` and the agent's binary is the whole allow list. Three things widen it: an attached provider adds its own endpoints, OpenShell drafts proposals from every blocked connection, and `proposal_approval_mode = auto` would approve those drafts. Keep approval mode `manual`. Put the Patchbay rule in the policy file, because a proposal that targets loopback cannot be approved. OpenShell can also inspect the gateway's traffic as MCP (`protocol: mcp`) and allow or deny tool names. |
| MCP server secrets stay in Patchbay's Keychain | **Feasible.** | Nothing in OpenShell needs them: the sandbox only talks to Patchbay. Patchbay's own bearer token is the one secret the sandbox needs. It can be a plain value written into the sandbox, which the agent can read. Better, it can be an OpenShell provider credential, which the agent sees only as a placeholder and which the supervisor swaps in on requests to the bound host and port. That needs a custom provider profile bound to `host.openshell.internal:7357`. The docs say substitution also works for plaintext HTTP requests. |
| Model keys in OpenShell's providers | **Feasible.** | Providers are the documented way. The agent's environment holds an opaque placeholder, and the supervisor substitutes the real key only on requests to the profile's endpoints. A `claude-code` example profile exists, and it uses an API key (`ANTHROPIC_API_KEY`/`CLAUDE_API_KEY`). |
| Skills get into the sandbox | **Feasible, as a copy; a live link is restricted.** | `openshell sandbox upload` and `sandbox create --upload` copy files into a running sandbox. Uploads keep symlinks as symlinks, so Patchbay's linked skill folders would arrive as dangling links. Upload the real folder (`~/.agents/skills/…`) to the path the agent reads, such as `/sandbox/.claude/skills`. A live bind mount works only after the gateway operator turns off a safety default (`enable_bind_mounts = true` with resource admission disabled). The MicroVM driver cannot mount a single file. |
| Playpen drives OpenShell from a GUI | **Feasible, with gaps.** | The gateway is a gRPC server, and Rust, Python, TypeScript and Go SDKs are published. Many CLI commands take `-o json`. There is no push stream of policy-advisor proposals at v0.1.9: the proto defines a `DraftPolicyUpdate` event, but no gateway code sends one. OpenShell's own TUI fetches proposals by calling `GetDraftPolicy`. `openshell rule get` has no JSON output, and the `rule` command is hidden from help. |

**Biggest unknown:** whether a supervisor in a Docker Desktop container reaches a service bound only to macOS
`127.0.0.1`. OpenShell assumes it does for its own gateway in Docker host mode, but no OpenShell source shows
Patchbay's case working. Test it, or test the MicroVM driver, where the code shows it.

---

## 1. Network

### What a sandbox's network is

- The workload has no direct network. Docker and Podman run it with networking turned off, Kubernetes allows only the
  supervisor service, and a MicroVM guest has no network device
  ([docs: Architecture, "How Each Runtime Builds the Boundary"](https://docs.nvidia.com/openshell/latest/about/architecture);
  `docs/about/architecture.mdx`). For Docker: "The workload always retains `network=none`"
  ([docs: Runtimes](https://docs.nvidia.com/openshell/latest/how-it-works/sandboxes/runtimes)).
- Every TCP open and DNS query is intercepted with seccomp and sent over an authenticated channel to the
  **supervisor**, which checks policy, adds credentials and "opens the real connection and relays traffic". The
  channel runs over a Unix socket for Docker and Podman, TCP for Kubernetes and vsock for MicroVM
  (`docs/about/architecture.mdx`, "Mediated Channel" and "How a network request travels").
- Where the supervisor runs:

  | Driver | Supervisor | Source |
  |---|---|---|
  | Docker | its own container, `host` network by default (`bridge` optional) | `docs/how-it-works/sandboxes/runtimes.mdx` |
  | Podman | its own container | same |
  | MicroVM (`vm`) | a process on the host | same, and `docs/about/architecture.mdx` |
  | Kubernetes | its own pod | same |

- A connection from the sandbox therefore leaves from the supervisor's network namespace, not the sandbox's.

### Can a sandbox reach the host, and at what address?

- **Reserved name.** Use `host.openshell.internal`. The docs use it for host services, for example an Ollama endpoint
  `host: host.openshell.internal`, `port: 11434`, with `OPENAI_BASE_URL=http://host.openshell.internal:11434/v1`. They
  also say: "Host-local services need `host.openshell.internal` or a reachable LAN/service hostname, not `127.0.0.1` or
  `localhost`" ([docs: Inference](https://docs.nvidia.com/openshell/latest/how-it-works/inference);
  `docs/how-it-works/inference.mdx` lines 137–173 and 303). The schema reference adds: "A rule for
  `host.openshell.internal` can still reach services on the gateway host"
  ([docs: Schema](https://docs.nvidia.com/openshell/latest/how-it-works/policies/schema); `schema.mdx` line 154).
- **Loopback is blocked for every other name.** "Network policy never authorizes an outbound endpoint whose
  destination is loopback, link-local, or unspecified … This does not prevent a workload from connecting to a
  sandbox-local service listening on loopback" (`schema.mdx` lines 147–155; also
  [docs: Network Rules, "Restrict Destination Addresses"](https://docs.nvidia.com/openshell/latest/how-it-works/policies/network-rules)).
  The code agrees. Tests in `crates/openshell-supervisor-network/src/proxy/destination.rs` (lines 421–488) say
  "loopback must be denied", "declared loopback must remain denied" and "loopback cannot be a trusted gateway". A
  policy rule for `127.0.0.1:7357` would never let anything through.
- **How the name is exempted.** `HOST_GATEWAY_ALIASES` lists `host.openshell.internal`, `host.containers.internal`
  and `host.docker.internal` (`crates/openshell-supervisor-network/src/proxy.rs` lines 274–282). When the compute
  driver supplies a host-gateway address, a connection to one of these names is authorized as
  `BackendPinnedGateway(ip)` (`destination.rs` lines 101–104). That check rejects only cloud-metadata addresses and
  addresses that differ from the pinned one (lines 242–255 and 341–357). A pinned `127.0.0.1` is allowed, and a test
  pins `127.0.0.1` (lines 587–601).
- **DNS for the name.** Mediated policy DNS starts with the driver's `host_gateway_ip`
  (`crates/openshell-supervisor-network/src/run.rs` lines 549–558). It answers a host-gateway name with that address,
  and only when a policy endpoint makes the name eligible (`policy_dns/mod.rs` lines 132–174).

### What each driver pins the name to

- **MicroVM:** `host_gateway_ip: Some(127.0.0.1)`, with the comment "The host-side control process is the network
  broker, so reserved host aliases terminate at its loopback address"
  (`crates/openshell-driver-vm/src/isolation/mod.rs` lines 134–137). The VM driver also rewrites the host aliases to
  `127.0.0.1` for its own endpoints, because "the supervisor runs on the host" (`crates/openshell-driver-vm/src/driver.rs`
  lines 200–205 and 1040–1047). **With this driver on a Mac, a rule for `host.openshell.internal:7357` reaches a
  server on macOS `127.0.0.1:7357`.** This was read from code, not run. Patchbay would not need to bind another
  interface.
- **Docker, host mode (the default):** when `host_gateway_ip` is unset and the mode is not bridge, the address is taken
  from the supervisor's gRPC endpoint (`crates/openshell-driver-docker/src/lib.rs` lines 972–985 and 5288–5299). That
  endpoint defaults to `http(s)://127.0.0.1:<gateway port>` (lines 7048–7051), so the name is pinned to `127.0.0.1` in
  the supervisor container's network, which on a Mac is Docker Desktop's Linux VM. The docs say: "Host mode on Docker
  Desktop requires host networking enabled" (`runtimes.mdx`; `crates/openshell-driver-docker/README.md` line 198).
  - *Inference:* the macOS Homebrew gateway listens on the Mac (`https://localhost:17670`, see Q3), and a host-mode
    supervisor reaches it at `127.0.0.1:17670`. So Docker Desktop's host networking presumably carries `127.0.0.1`
    from the container to macOS loopback, and Patchbay on `127.0.0.1:7357` would be reached the same way. No
    OpenShell source says this about other host services, and this was not tested.
- **Docker, bridge mode:** the driver reads Docker's `host-gateway` address from a throwaway container's `/etc/hosts`
  and pins the name to it. Loopback is rejected as a `host_gateway_ip` in bridge mode (`lib.rs` lines 7071–7090,
  7092–7160; `README.md` lines 198–221). The docs say that "On native Linux, the gateway and host services must listen
  on bridge-reachable addresses" (`runtimes.mdx`). For Docker Desktop they say only that bridge mode "uses
  `host.docker.internal`". Whether that reaches a service bound only to macOS loopback depends on Docker Desktop; no
  OpenShell source says.
- **Podman on macOS:** the default `host_gateway_ip` is `192.168.127.254`, "Trusted supervisor-side destination
  returned for host.openshell.internal" (gvproxy) (`docs/how-it-works/gateways/configuration.mdx` lines 1648–1651;
  `crates/openshell-driver-podman/src/config.rs` lines 524 and 727–733). Reachability of macOS loopback through it
  is not stated.
- **Driver choice:** "When `compute_driver` is unset, the gateway auto-detects Kubernetes, then Podman, then Docker.
  The VM driver is never auto-detected" (`runtimes.mdx`). A default Mac install with Docker Desktop therefore uses
  Docker. MicroVM needs `compute_driver = "vm"` in `gateway.toml`.

### How network policies are written

- **Format.** A YAML file with `version: 1` and up to five other sections: `filesystem_policy`, `landlock`,
  `process`, `network_policies` and `network_middlewares`. Only the last two can change while the sandbox runs
  ([docs: Policies overview](https://docs.nvidia.com/openshell/latest/how-it-works/policies/overview); `schema.mdx`
  lines 15–37). The CLI's `policy set` also accepts the JSON form (`manage-policies.mdx` lines 165–178).
- **Rules.** `network_policies` is a map of named rules. Each rule has `endpoints` and `binaries`, and allows every
  listed binary to reach every listed endpoint. Rules are not ordered. Matching allows add together, and a matching
  deny wins. Keys starting `_provider_` are reserved (`network-rules.mdx`, "Rule Structure" and "Overlapping Rules";
  `schema.mdx` lines 108–121).
- **Endpoint fields.** `host` (a name, an IP or a wildcard), `port` or `ports`, `path`, `allowed_ips`, and
  `protocol`. The protocol is one of `rest`, `websocket`, `graphql`, `mcp`, `json-rpc` or `tcp`. Endpoints also take
  `tls` (`skip`), `enforcement` (`enforce` or `audit`, default `audit`), `access` (`read-only`, `read-write` or
  `full`), `rules`, `deny_rules` and `allow_encoded_slash`, plus credential and protocol options (`schema.mdx` lines
  123–232).
- **Wildcards.** A wildcard host needs at least three DNS labels. `*` may appear inside the first label or as a whole
  later label, and `**` only as the whole first label. Wildcard and hostless endpoints reach private addresses only
  through `allowed_ips` (`schema.mdx` lines 137–146). Matching is described under "Matcher Semantics" (`schema.mdx`
  lines 576–605).
- **Binaries.** `path` is the real executable path, or a glob such as `/usr/lib/jvm/*/bin/java`. A rule also covers
  children of a listed binary. Scripts are matched by their interpreter. A rule with an empty `binaries` list allows
  nothing. A hash of each executable is recorded on first use, and later connections are denied if the file changes
  (`network-rules.mdx`, "Binary Matching"; `schema.mdx` lines 525–537). The docs do not say whether a catch-all such
  as `/**` is accepted.
- **Blocked ports.** Ports 2379, 2380, 6443, 10250 and 10255 are always blocked for exact, IP and `allowed_ips`
  endpoints (`schema.mdx` line 152).

### Allowing one host:port and nothing else

Policy denies by default, so "nothing else" means the file lists nothing else. For Patchbay, with the agent's real
binary path:

```yaml
version: 1
network_policies:
  patchbay:
    endpoints:
      - host: host.openshell.internal
        port: 7357
    binaries:
      - path: /usr/local/bin/claude   # the real path in the image; `readlink -f` it
```

Apply it at creation with `openshell sandbox create --name <name> --policy ./policy.yaml -- <agent>`, or set
`OPENSHELL_SANDBOX_POLICY`. On a running sandbox, replace it with
`openshell policy set <name> --policy policy.yaml --wait`, or add just the rule with
`openshell policy update <name> --rule-name patchbay --binary /usr/local/bin/claude --add-endpoint host.openshell.internal:7357 --wait`.
The `--add-endpoint` value takes the form `host:port[:access[:protocol[:enforcement[:options]]]]`
(`manage-policies.mdx` lines 16–35, 84–134, 136–181; `crates/openshell-cli/src/policy_update.rs` lines 409–439).

Notes on that rule:

- With no `protocol`, any method and path is allowed. OpenShell still parses HTTP and rejects requests addressed to
  another host (`network-rules.mdx`, "Connection and Request Checks").
- To inspect the traffic as MCP, add `protocol: mcp` with `rules`, or set `mcp.allow_all_known_mcp_methods: true`.
  Tool names can then be allowed or denied (`tool:` matcher), but tool arguments are not checked. Only MCP revision
  `2025-11-25` is accepted unless `mcp.versions` lists others, and responses are not inspected (`network-rules.mdx`,
  "Allow MCP Tools"; `schema.mdx` lines 340–430). MCP rules need `policy set`, not `policy update`
  (`manage-policies.mdx` lines 138–141).
- Other things that add reach: an attached provider contributes its own rules to the "effective policy". For
  example, the `claude-code` example profile adds `api.anthropic.com`, `statsig.anthropic.com` and `sentry.io`
  (`providers/claude-code.yaml`). A gateway-wide global policy replaces the sandbox's own policy
  ([docs: Policies overview](https://docs.nvidia.com/openshell/latest/how-it-works/policies/overview)). Inspect the
  effective policy with `openshell policy get <name> --full`.
- When a filesystem policy is present, the effective policy also adds baseline read paths (`/usr`, `/lib`, `/etc` …)
  whenever any network rule exists
  ([docs: Default Policy](https://docs.nvidia.com/openshell/latest/how-it-works/policies/default-policy), lines 58–75).

### How the policy advisor proposes rules

([docs: Policy Advisor](https://docs.nvidia.com/openshell/latest/how-it-works/policies/advisor);
`docs/how-it-works/policies/advisor.mdx`)

- **Off by default.** Turn it on with the setting `agent_policy_proposals_enabled`:
  `openshell settings set --global --key agent_policy_proposals_enabled --value true --yes`, or per sandbox.
- **What the agent gets when it is on:** a guide at `/etc/openshell/skills/policy_advisor.md`, a skill at
  `/etc/openshell/skills/policy-advisor/SKILL.md`, and `/AGENTS.md` if the image has none. It also gets an HTTP API at
  `http://policy.local`:

  | Route | Use |
  |---|---|
  | `GET /v1/policy/current` | read the current policy |
  | `GET /v1/denials?last=N` | read recent denials |
  | `POST /v1/proposals` | submit a proposal |
  | `GET /v1/proposals/{chunk_id}` | read a proposal's status |
  | `GET /v1/proposals/{chunk_id}/wait?timeout=300` | wait for a decision |

- **Drafts without the advisor.** Even with the advisor off, OpenShell drafts a proposal for each blocked connection:
  one binary to one host and port.
- **Checks.** Each proposal goes through the policy prover's risk check, which reports `link_local_reach`,
  `l7_bypass_credentialed`, `credential_reach_expansion` and `capability_expansion`. Destination flags cover private
  IPs, wildcards, private `allowed_ips`, ports above 49152, and database or cache ports.
- **Review.** Proposals wait for review: `openshell rule get <sandbox> --status pending`,
  `openshell rule approve <sandbox> --chunk-id <id>`, `openshell rule reject <sandbox> --chunk-id <id> --reason "…"`,
  and `openshell rule approve-all` (add `--include-security-flagged` to include flagged ones).
- **Automatic approval.** `proposal_approval_mode` takes `manual` (the default) or `auto`. In `auto`, proposals with
  no findings and no flags are approved, which includes new public hosts.
- **Limits.** Agents cannot propose `protocol: tcp`, `tls: skip`, MCP tool rules or credential settings. "Loopback,
  link-local, and cloud metadata addresses are always blocked, and you cannot approve a proposal that targets them."
- **Logs.** The log shows `CONFIG:PROPOSED`, `CONFIG:APPROVED`, `CONFIG:REJECTED` and `CONFIG:LOADED`.

### How approved rules hot-reload

- Network rules and middleware are live. The gateway validates the change and saves a revision. The sandbox
  validates it again and loads it, then closes every connection opened under the old rules, including keep-alive
  connections, tunnels, WebSockets and streams. If the sandbox fails to load a revision,
  `policy_validation_failure_mode` decides what happens: `fail_closed` (the default) blocks traffic, and
  `retain_last_valid` keeps the last valid policy ([docs: Manage Policies, "How Changes Take
  Effect"](https://docs.nvidia.com/openshell/latest/how-it-works/policies/manage-policies); `manage-policies.mdx`
  lines 183–221).
- After an approval, `/wait` reports `policy_reloaded: true` once the rule is loaded (`advisor.mdx`, "Agent API").
- `openshell policy list <name>` shows each revision and whether the sandbox loaded it.

---

## 2. Files

- **Copying in and out.** `openshell sandbox upload <name> <local> [dest]` and
  `openshell sandbox download <name> <src> <local>` work on a running sandbox, at any time
  ([docs: Sandboxes, "Transfer Files"](https://docs.nvidia.com/openshell/latest/how-it-works/sandboxes/overview);
  `overview.mdx` lines 912–987).
  - `sandbox create --upload <local>:<dest>` uploads at creation and can be repeated, but it "cannot yet be combined
    with a trailing main command because uploads finish after the canonical process starts" (lines 77–80, 956–963).
  - Inside a Git work tree, uploads skip `.gitignore`d files unless `--no-git-ignore` is passed.
  - **Symlinks are preserved as symlinks**, dangling ones included (line 936).
  - A single file can be renamed on upload: `openshell sandbox upload my-sandbox ./config.json /sandbox/settings.json --no-git-ignore`
    (line 938).
  - Downloads are limited to the sandbox's working directory. The docs state no such limit for upload destinations.
    *Inference:* the upload runs inside the sandbox as the sandbox user, so Landlock decides where it can write.
- **Paths an agent's config lands on.** The default sandbox user is `sandbox` with `HOME=/sandbox`, unless the policy
  names another user or the workspace is used as home (`crates/openshell-sandbox/src/process.rs` lines 190–210).
  - Kubernetes and MicroVM always use `/sandbox` as the workspace. On Docker and Podman, the image's `WORKDIR` becomes
    the workspace, falling back to `/sandbox` (`runtimes.mdx`, "Sandbox User Identity").
  - `~/.claude.json` and `.mcp.json` would therefore go to `/sandbox/.claude.json` and the project folder, and skills
    to `/sandbox/.claude/skills/…`. These paths are inferred from the agents' conventions; OpenShell does not name
    them.
- **Environment.** `--env KEY=VALUE` and `--env-from KEY[=ENVVAR]` set variables that every process can read. The
  limits are 128 variables, 8 KiB each and no newlines. The docs advise providers for secrets (`overview.mdx` lines
  389–423).
- **Mounts (the "volume" concept).** Mounts are passed per sandbox through `--driver-config-json`. Driver config is
  disabled unless the operator sets `allow_driver_config = true` (`runtimes.mdx`, "Pass Driver Config").
  - **Docker and Podman** take `volume` mounts (named volumes labelled `openshell.ai/sandbox-attachable=true`),
    `tmpfs`, and `bind` (an absolute host path). Podman also takes `image` mounts.
  - **Bind mounts** need `enable_bind_mounts = true` and `[openshell.drivers.docker.resource_admission] enabled = false`.
    The docs warn that they "can bypass workspace isolation and filesystem policy".
  - **MicroVM** shares host directories over virtiofs: up to 32 on arm64, `read_only` by default, the same opt-in.
    "Single-file sources, sources that are symbolic links … are not supported."
  - **Kubernetes** mounts existing PVCs.
  - Mount targets cannot replace the workspace root or `/opt/openshell`, `/etc/openshell` and `/run/openshell`
    (`runtimes.mdx`, "Docker Mounts", "Podman Mounts", "MicroVM Mounts" and "Kubernetes PVC Mounts").
  - The docs describe mounts only at creation; nothing says one can be added to a running sandbox.
- **"Workspace" has two meanings.** An OpenShell *workspace* is a tenancy boundary for sandboxes, providers and
  policies. It is "separate from the `/sandbox` filesystem directory"
  ([docs: Workspaces](https://docs.nvidia.com/openshell/latest/how-it-works/workspaces), lines 11–18). Persistence
  differs by driver: Docker and Podman custom workspaces live in the container's writable layer, and survive
  stop/start but not deletion (`runtimes.mdx`, last paragraph). Kubernetes uses a workspace PVC.
- **Filesystem policy.** `filesystem_policy` has `include_workdir`, `read_only` and `read_write`.
  - Paths must be absolute, at most 256 of them, and `read_write` cannot contain `/`. The policy is enforced by
    Landlock (ABI 3 or newer). `landlock.compatibility` is `best_effort` (the default) or `hard_requirement`.
  - With no `filesystem_policy`, the working directory is read-write (`schema.mdx` lines 39–88).
  - The default policy also grants read-only system paths (`default-policy.mdx`).
- **Changing it while running.** It cannot be changed. "Filesystem, Landlock, and process settings are fixed once the
  sandbox starts." Added paths are saved but not applied, and removed paths are rejected. Both need a new sandbox
  (`overview.mdx` of Policies; `manage-policies.mdx` lines 189–195). Uploading into paths that are already writable
  works at any time.
- **Images.** Any standard Linux OCI image, or a rootfs tar, given with `--from`. A policy can ship in the image at
  `/etc/openshell/policy.yaml` (`overview.mdx` lines 179–247; `manage-policies.mdx` lines 40–51). Baking skills or a
  config into the image is the other way in.

---

## 3. Platforms

- **Where it runs**
  ([docs: Support Matrix](https://docs.nvidia.com/openshell/latest/about/support-matrix); `support-matrix.mdx`):

  | Platform | Status |
  |---|---|
  | Linux (Debian/Ubuntu) amd64 and arm64 | supported |
  | macOS (Docker Desktop) Apple Silicon | supported |
  | Windows (WSL 2 + Docker Desktop) | experimental |
  | native Windows (MXC) | a separate `windows` branch only |

  - Intel Macs are not listed. Gateway images are `linux/amd64` and `linux/arm64`.
  - The standalone gateway binary is published for `aarch64-apple-darwin`.
  - The sandbox runtime binary is published only for Linux, as static musl builds.
- **On macOS**
  ([docs: Installation, "macOS"](https://docs.nvidia.com/openshell/latest/about/installation)):
  - The gateway runs natively, as a Homebrew service at `https://localhost:17670`.
  - It reads `~/.config/openshell/gateway.toml`, or `$(brew --prefix)/var/openshell/gateway.toml`.
  - It is installed by `curl -LsSf https://raw.githubusercontent.com/NVIDIA/OpenShell/main/install.sh | sh`.
  - The Homebrew formula also installs and ad-hoc signs `openshell-driver-vm` with the
    `com.apple.security.hypervisor` entitlement (`tasks/scripts/release.py` lines 341–361 and 455–467).
  - Sandboxes are Linux in all cases. "On macOS, these kernel modules run inside the Docker Desktop Linux VM, not on
    the host kernel" (`support-matrix.mdx`, "Kernel Requirements").
- **Runtimes:**

  | Runtime | Status | Needs |
  |---|---|---|
  | Docker | supported for local and single-machine use | Docker 28.0 or newer |
  | Podman | supported for rootless use | Podman 5.x |
  | Kubernetes | supported, through the Helm chart | Kubernetes 1.29 or newer |
  | MicroVM | supported | libkrun; Hypervisor.framework on macOS, KVM on Linux |

  Windows MXC is in development. ([docs: Runtimes](https://docs.nvidia.com/openshell/latest/how-it-works/sandboxes/runtimes);
  `support-matrix.mdx`, "Runtimes" and "Software Prerequisites".)
- **Sandbox technology.** A container, pod or microVM boundary, inside which:
  - **Landlock**, ABI 3 or newer (Linux 6.2 or newer), limits files.
  - **seccomp** user notification intercepts network operations.
  - There is one non-root identity and no Linux capabilities.
  - Workload sockets are bound to loopback with `SO_BINDTODEVICE`.
  - The sandbox probes these before it admits the workload, and fails closed (`support-matrix.mdx`, "Kernel
    Requirements"; `architecture.mdx`, "Inside the Sandbox Boundary").

  gVisor is not mentioned. Kata Containers appears only as a Kubernetes `runtime_class_name` example given through
  driver config (`runtimes.mdx`).

---

## 4. Interfaces

- **"The gateway"** in OpenShell's terms is the control plane, not a network gateway.
  - It provisions sandboxes through a compute driver.
  - It stores provider credentials.
  - It delivers policy and settings.
  - It runs the policy prover.
  - It brokers exec, SSH, logs and service forwarding. Enforcement happens in each sandbox's supervisor.
  ([docs: Gateways](https://docs.nvidia.com/openshell/latest/how-it-works/gateways/overview);
  `architecture.mdx`, "What Each Piece Does".) It is not related to Patchbay's MCP gateway.
- **API.** The gateway is a gRPC server (`openshell.v1.OpenShell`, defined in `proto/openshell.proto`) and serves gRPC
  reflection.
  - The local gateway uses mTLS by default. The CLI's client bundle is in
    `~/.config/openshell/gateways/<name>/mtls/` (`ca.crt`, `tls.crt`, `tls.key`)
    ([docs: Authentication](https://docs.nvidia.com/openshell/latest/how-it-works/gateways/authentication)).
  - OIDC and edge authentication are the alternatives.
- **SDKs.**

  | SDK | Where it comes from |
  |---|---|
  | Rust (`openshell-sdk`) | consumed from git at a release tag; the CLI and TUI use it |
  | Python (`openshell` on PyPI) | can reuse the CLI's gateway registration |
  | TypeScript (`@nvidia/openshell-sdk`) | GitHub Packages |
  | Go (`github.com/ai-openshell/sdk-go`) | its own repository |

  ([docs: SDK pages](https://docs.nvidia.com/openshell/latest/sdk/rust); `docs/sdk/*.mdx`.)
- **Rust SDK and mTLS, a contradiction.** `docs/sdk/rust.mdx` line 60 says the Rust SDK "does not support mTLS client
  certificates or read the CLI gateway configuration from disk". The code at the same tag has
  `ClientConfig.client_identity: Option<ClientIdentity>`, "Client certificate and private key for mutual TLS"
  (`crates/openshell-sdk/src/config.rs` lines 111–113). It is wired into tonic TLS
  (`crates/openshell-sdk/src/transport.rs` lines 133–134). So mTLS appears to be supported, but reading the CLI's
  bundle from disk is left to the caller.
- **JSON output.**
  - `--output json`, `-o json` or `yaml` is documented for:
    - `status`, `gateway list` and `gateway info`
    - `sandbox create`, `sandbox list` and `sandbox get`
    - `provider list` and `provider update --wait`
    - `policy get` (`--output json`)
    - `service list` and `forward list`
  - **`openshell rule get` has no output flag.** Its arguments are only `name` and `--status`. The `rule` command (alias
    `rl`) is declared `hide = true`, so it does not appear in help even though the docs use it
    (`crates/openshell-cli/src/main.rs` lines 578–583 and 2100–2161).
- **Watching for proposals.**
  - **No push stream at v0.1.9.** `WatchSandbox` streams `SandboxStreamEvent`s: sandbox snapshots, log lines, platform
    events, warnings, and a `DraftPolicyUpdate` payload ("Draft policy update notification", `proto/openshell.proto`
    lines 667–679, 2151–2236 and 3639–3649). But no code at the tag constructs a `DraftPolicyUpdate`. The only
    references are where the CLI and Rust SDK ignore it (`crates/openshell-cli/src/run.rs` line 1017;
    `crates/openshell-sdk/src/client.rs` line 1648).
  - **What works:**
    - Call `GetDraftPolicy`, the RPC behind `rule get`, along with `ApproveDraftChunk`, `RejectDraftChunk`,
      `ApproveAllDraftChunks`, `EditDraftChunk`, `UndoDraftChunk`, `ClearDraftChunks` and `GetDraftHistory`
      (`openshell.proto` lines 694–765). The TUI refreshes its proposal counts and lists this way
      (`crates/openshell-tui/src/lib.rs` lines 2995–3084).
    - Follow logs (`openshell logs <name> --tail`, or `WatchSandbox` with `follow_logs`) for `CONFIG:*` events
      (`advisor.mdx`, "Logs").
    - `openshell term` is a TUI that shows proposals and can approve or reject them.

---

## 5. Also

- **MCP.** OpenShell has no registry or management of MCP servers. It knows MCP as a **wire protocol to police**:
  - `protocol: mcp` endpoints inspect Streamable HTTP requests by method and tool name, for revisions `2025-03-26`,
    `2025-06-18`, `2025-11-25` and `2026-07-28` (`schema.mdx` lines 340–507).
  - `sandbox get` reports "Tool server connections" (`endpoint_statuses`) for configured MCP-over-HTTP endpoints
    (`overview.mdx` lines 740–773).
  - Server responses are not inspected.
- **Credential injection.** The agent's environment holds an opaque placeholder. When an HTTP request carries it, the
  supervisor substitutes the real value, and only if all three checks pass: the network policy allows the binary and
  destination, the credential is bound to that host, port and path, and the placeholder sits in the declared header,
  query parameter or path slot (`Authorization` by default)
  ([docs: Providers, "How Credential Injection Works"](https://docs.nvidia.com/openshell/latest/how-it-works/providers/overview);
  `overview.mdx` lines 447–507).
  - Request bodies and WebSocket text are rewritten only when the endpoint enables it. `tls: skip` and non-HTTP
    traffic get no injection.
  - Substitution covers "plaintext HTTP forward-proxy requests" too
    ([docs: Profiles](https://docs.nvidia.com/openshell/latest/how-it-works/providers/profiles), line 651).
  - `stable_placeholder: true` lets an outside service rotate a credential, with the example of "an MCP client …
    caller-assertion header", without the agent restarting (`profiles.mdx` lines 838–850).
  - Gateway-side refresh supports OAuth2 refresh tokens, OAuth2 client credentials, Google service accounts and AWS
    STS.
  - An mTLS **operator** can read real values through `GetProviderCredentials`.
  - Plain `--env` values are visible to the agent, and the CLI warns about names that look like secrets.
- **Licence and source.** Apache License 2.0 (`LICENSE`). The source is public at
  https://github.com/NVIDIA/OpenShell, a Rust workspace with crates for the CLI, gateway, supervisor, sandbox,
  drivers, prover, TUI and SDK. The Go SDK is in https://github.com/ai-openshell/sdk-go.

## Tested on this Mac, 2026-11-19

OpenShell 0.1.8 from Homebrew (one patch behind the version read above), its gateway as a Homebrew service, no
`compute_driver` set, so sandboxes ran on Docker, which here is **OrbStack**, not Docker Desktop. The image was the
default, `nvcr.io/nvidia/base/ubuntu:24.04`, which has bash and Perl but no curl or Python. The policy:

```yaml
version: 1
network_policies:
  patchbay:
    endpoints:
      - host: host.openshell.internal
        port: 7399
    binaries:
      - path: /usr/bin/bash
```

What happened, each from a fresh `openshell sandbox create --no-keep --policy … -- <command>`:

- **A server bound only to macOS `127.0.0.1:7399` is reachable** as `host.openshell.internal:7399`, with bash's
  `/dev/tcp`. The server saw the request come from `127.0.0.1`. So on OrbStack the open question in Q1 is settled: a
  host service can stay on loopback.
- **Nothing else is.** From the same sandbox, `host.openshell.internal:17670` (OpenShell's own gateway) and
  `api.github.com:443` were refused with `Permission denied`, and `127.0.0.1:7399` with `Connection refused`.
- **A rule is held to its binary.** Perl run as the sandbox's own command was refused the same host and port.
  Perl started from bash got through, as the docs say: a rule covers a listed binary's children. So a rule naming an
  agent's shell or interpreter covers whatever it runs.
- **Patchbay's gateway answers MCP from the sandbox once it accepts the name.** rmcp's HTTP server only accepts a
  `Host` of `localhost`, `127.0.0.1` or `::1`, and answered `403` to `Host: host.openshell.internal:7399`. Sending
  `Host: 127.0.0.1:7399` instead is refused by OpenShell itself (`403`, even with a wrong token), since it checks a
  request's Host against where it's going. With `host.openshell.internal` added to the gateway's allowed hosts
  (`StreamableHttpServerConfig::with_allowed_hosts`, a local change not kept), bash in the sandbox initialized an
  MCP session, listed Patchbay's tools (`fetch__fetch`), and a wrong token still got `401`.

So for Patchbay the change is small: accept `host.openshell.internal` as a Host (opt-in, as LAN access is planned),
and give sandbox clients an entry that uses that name. Docker Desktop and the MicroVM driver were not tried.

## Not established by these sources

- Whether Docker Desktop host networking, or bridge mode through `host.docker.internal`, carries a sandbox
  supervisor's connection to a service bound only to macOS `127.0.0.1`. Podman's `192.168.127.254` has the same open
  question.
- Whether a binary glob that matches everything is accepted.
- Whether mounts can be added to a running sandbox. The docs describe them only at creation.
- How often, if at all, `DraftPolicyUpdate` will be emitted in later releases.
