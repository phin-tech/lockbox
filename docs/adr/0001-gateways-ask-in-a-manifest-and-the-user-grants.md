# Gateways ask in a Manifest, and the user grants

Where a sandbox runs is an OpenShell gateway: the service that keeps sandboxes and runs them on a runtime of its own
(Docker, Podman, Kubernetes, MicroVM). This Mac's Docker is already one, a local gateway. Playpen used only the active
one. It names the gateway in every call it makes, and lets a person have several: one on this Mac, one in a Kubernetes
cluster, one on an EC2 machine. Playpen needs no second implementation of sandboxes. What a gateway still needs from
Playpen is an agent's image, which comes by an image source (below).

The gateway is named with `OPENSHELL_GATEWAY`, the setting behind `--gateway`, and not with the flag. A variable also
reaches the shell line Playpen hands to a terminal, set there with `env` so that it works in any shell, fish included.
With no name a call goes to the active gateway, as before.

Every gateway in `openshell gateway list` is usable as it is. A person who reaches one through a VPN, a jump host or a
tunnel of their own registers it with `openshell gateway add`, and Playpen uses it. It never starts, stops or configures
a network, and has no Capability for that. A gateway that doesn't answer shows as unreachable, with a retry, and the
others carry on; each gets its own short timeout. Playpen's `Place` already names where a terminal shows a session, so
that word isn't used for this.

A gateway can also come from a **Gateway extension**: a folder holding a Manifest (`gateway.toml`) and JavaScript, run in
the same kind of QuickJS sandbox as Microfiche's Extensions. It lists its `gateways()`, makes one reachable with
`ensure(gateway)`, names the image for an agent with `image(agent)`, and may `release(gateway)`. It returns plain data,
and Playpen shows its progress the way it shows making a sandbox.

## Image sources

An agent's image has to be somewhere the gateway can start it. Playpen asks an **image source** for it, which makes it
ready and says the reference to give `--from`:

- **Local Docker,** today's: built with Docker on this Mac when it isn't there, and named as it is now. It suits a gateway
  that shares this Mac's Docker.
- **A registry:** the image is built and pushed under a prefix the person gives, and named by it
  (`registry.example.com/team/playpen-codex:0.171.0`), or already there from CI. It suits a cluster, which can't pull
  from this Mac. A pushed image is checked for before it is built again.
- **Left to the person,** for a gateway Playpen can't push to: Playpen names the image it needs, builds and pushes
  nothing. OpenShell's own error says when the gateway can't find it.
- **Built by the Cloud** from the Dockerfile Playpen holds, which only Lambda MicroVMs does (it takes a zip uploaded to S3
  and not a pushed image). That one is parked with it.

An image is for one **platform**, and a gateway's may not be this Mac's: Apple Silicon builds `linux/arm64`, and a cluster
is often `linux/amd64`. The platform is part of the image source, and what depends on it follows it. Codex's policy names
its program by a path that differs by platform, so a policy is written for the platform its image is built for.

## Organised by Cloud

A Gateway extension is for one **Cloud** and offers several **kinds** of Gateway on it: `aws` offers `ec2` and `eks`, and
`gcp` offers `gce` and `gke`. This Mac, and every gateway registered by hand, belong to no Cloud and need no extension.
The Manifest lists the kinds, and `gateways()` returns the gateways of each. A Cloud is also the unit its Capabilities
are named by (`aws.ec2.start`), its Grant is given for (one profile and region), and Settings ▸ Gateways and New
Agent…'s picker are grouped by. A kind that can't be used yet, because OpenShell has no driver for it, is listed as not
available and doesn't stop the Cloud's others. Which image source a kind uses is part of its description.

## Kubernetes first

The first Gateway beyond this Mac is a **Kubernetes** one: an OpenShell gateway installed in a cluster, with the
Kubernetes compute driver OpenShell ships. Playpen needs no driver of its own and never talks to the Kubernetes API. It
talks to the gateway, which the person installs with OpenShell's Helm chart and registers with
`openshell gateway add`; from then on it's a Gateway like any other. Sandbox lifetime, stop and start (suspend maps to
Playpen's `Suspended`), exec, file upload and logs all go through the gateway, so they work as they do here.

What differs from this Mac, and what Playpen has to handle:

- **The image has to be pullable by the cluster.** Building with local Docker is no use, so a Kubernetes Gateway's image
  source is a registry: Playpen builds and pushes, or names an image CI already pushed. Image-pull Secrets belong to the
  gateway's configuration, never to a sandbox.
- **The cluster has to enforce NetworkPolicy.** The driver writes the policy that denies a workload's egress, but
  Kubernetes accepts one without confirming the CNI enforces it. Playpen can't check this, so the docs for a Kubernetes
  Gateway say to verify it before running a sandbox.
- **Reaching the gateway.** Ingress is a Gateway API `GRPCRoute`, and a user authenticates with mTLS (a client
  certificate the chart generates) or OIDC. The first connection of an OIDC gateway opens a browser.
- **`host.openshell.internal` is the cluster's,** not the Mac's.
- **It needs the Agent Sandbox controller** (`agents.x-k8s.io`) and a default StorageClass, or the sandbox's `/sandbox`
  volume stays pending.

A Gateway extension for Kubernetes is optional and comes later: installing the chart, or reaching a private cluster, is
what one would do. A Kubernetes Gateway isn't grouped under a Cloud, since a cluster needn't belong to one.

## Lambda MicroVMs

Parked. As its guide describes it, a Lambda MicroVM is a Gateway only if an external compute driver is written for it (a
separate service beside a gateway, over a Unix socket), and only if that driver can fence the guest's network, which
has internet by default. Its lifespan is capped at eight hours, running and suspended, which is shorter than a sandbox
is meant to last. The kind is listed as not available.

## Capabilities and Grants

An extension reaches nothing but what the host does for it. Each thing the host can do is a **Capability**, typed and
named (`aws.ec2.start`, `kube.port-forward`, `gateway.add`), with parameters the host checks. The Manifest only asks:
it lists the Capabilities it needs, with the limits it needs them at. The user's **Grant** is a copy of exactly what was
asked when they allowed it. It's kept in `grants.toml` beside `settings.toml`, outside the extension's folder, because
an agent can edit anything inside it, and a file that could widen its own reach would give no guarantee. It is keyed by
the folder's path and the extension's name, and it records a hash of the Manifest and the code. A change to either
asks again, and shows what's new. The default is none.

The host checks every call against the Grant, performs it with a clean environment holding only the variables granted,
and holds every credential, so a key never reaches JavaScript. An extension never sees a model key either. Those are
OpenShell's Providers, which a gateway keeps (see Consequences).

The Grant is shown in plain words: "start EC2 instances in us-east-1, and add gateways named `ec2-*`". Settings gains a
Gateways section, built on Typecase's extensions settings, with Trust and Revoke for the person; code and docs say Grant.

## Adding a Capability

A Capability is host code: a Rust implementation in `playpen-core`, a typed parameter schema, a sentence the Grant is
shown in, and a test that a call outside a Grant is refused. It ships with a Playpen release, since the app bundles one
binary. An extension can't define one, and that is the point.

- A new Capability grants nothing to existing extensions, since the default is none.
- A Manifest naming a Capability this Playpen doesn't know isn't run. It's shown as needing a newer Playpen.
- A Capability's meaning never changes under a Grant; a changed one gets a new name (`aws.ec2.start.v2`).
- A Manifest marks each Capability `required` or `optional`. A missing optional one runs the extension in a reduced form.
- Someone who needs another asks for it, and the host code is reviewed. That review is the boundary.

There is no catalogue to design first. Playpen starts with the two a real Gateway needs, `gateway` and `exec`, and adds a
typed one when a Gateway being written shows a Grant that is too wide, such as `aws ec2 *` wanting to be
`aws.ec2.start`. The list grows from Gateways that exist.

A generic `exec` Capability, a granted pattern of argv, is available for a person's own extensions. It is off by
default, marked as full trust where it's granted, and left out of anything installed from someone else, since a pattern
such as `aws *` allows whatever the credentials do.

## Considered Options

- **A compiled-in trait, one Rust type for each Gateway.** Rejected: each new Gateway needs a release, and nobody else can
  add one.
- **A dynamic library.** Rejected: unsafe code is denied in the workspace, a library has to be signed and notarized to
  load, and `release.toml` bundles one binary.
- **An executable found on `PATH`, like `git-credential-*`.** Rejected: a Grant can only say whether it runs, not what
  it does, since it has the user's full authority.
- **WebAssembly now.** Rejected for now. It isolates better and meters CPU, but costs Microfiche's runtime, Manifest and
  Settings part, a build step for authors, and a heavy dependency, for a threat that doesn't exist while the extensions
  are a person's own or an agent's. The contract is plain JSON and a small set of host functions, with nothing specific
  to QuickJS, so the engine can change without changing an extension.
- **Reading the gateways from OpenShell and keeping no list of our own.** Kept: `openshell gateway list` is the
  registry of where a gateway is, and an extension's `ensure` is what registers or selects one.

## Consequences

Extensions from strangers are not accepted until two things are done: typed Capabilities replace `exec` in anything
they can ask for, and the runtime is WebAssembly. A QuickJS runtime in-process is fine for code the person or their
agent wrote, and is not a boundary against code that means harm.

A remote Gateway is a different host. `host.openshell.internal` there is the gateway's host, not the Mac, so anything in
a policy or MCP config that reaches a service on the Mac doesn't carry over, and a folder copied in comes from the Mac
by upload, as it does now.

**Providers belong to a gateway.** OpenShell keeps a Provider, with its key and profile, in the gateway that holds it, so
a second gateway starts with none: a sandbox in a cluster uses a Provider that is in the cluster's gateway, stored by its
credential driver (a database, Kubernetes Secrets or Vault). Adding, rekeying and deleting a Provider, and signing in to
ChatGPT for Codex, then act on one gateway, and Settings has to say which. A person adds the key again for each gateway.
Playpen doesn't copy a key from one to another.

**What exists and what is next.** A Gateway type names where an `openshell` call goes, and opening a session carries it
(done). Making a sandbox asks an image source for the image and no longer calls `docker` itself (done): this Mac's
Docker, a registry, or an image the person provides, with the platform the image is for. Nothing chooses a source but
the local one yet. Next, a sandbox records which gateway it is in, so Open, Stop and Delete go to its own, with a picker
and a list across gateways, and the choice of image source goes with each gateway.

New dependency `rquickjs`, unless the runtime is shared with Microfiche or Typecase first, needs its licence in
`deny.toml` (`just lint`). That is still to be checked.
