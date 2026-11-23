# Gateway health: who you are, what it runs, and signing out

The sidebar shows a gateway as reachable or not, from whether listing its sandboxes worked. OpenShell can say more, and
all of it is cheap to ask.

## What the person sees

Clicking a gateway's row (or **Gateway ▸ Show Info**, `gateway.info`) shows:

- **Connected as**, from `whoami -o json`: the identity the gateway validated. On a local mTLS gateway it says so. On an
  OIDC gateway it's the person's account, which is what a team gateway's roles hang on.
- **Version**, from `status -o json`. If it differs from the `openshell` on this Mac, say so, since a weekly release
  cadence makes that common ([support matrix](https://docs.nvidia.com/openshell/v0.1.9/about/support-matrix)).
- **Runs on,** from `gateway info -o json`: its compute drivers (`docker`, `podman`, `microvm`, `kubernetes`) and
  credential driver. This is also where plans 4 and 7 look for what a gateway can do.
- **Sign Out** (`gateway.logout`, `gateway logout`) beside the existing Sign In Again, for OIDC and edge gateways only.
  Signing in afterwards asks the identity provider afresh, so this is how a person changes accounts.

## Checking this Mac

When the local gateway doesn't answer, `doctor check` says whether a Docker-compatible runtime is installed, running and
reachable, with its socket path. Show its result in place of the plain "not reachable" for the local gateway only, with
the next step: start OrbStack or Docker Desktop.

## How it's built

- Core: `whoami`, `status` and `gateway_info`, each typed from JSON, plus `logout` and `doctor`. Ask for them when a
  gateway's info is shown, not every few seconds.
- These run with `OPENSHELL_GATEWAY` set, like every other call. `status` against a closed gateway is never asked, per
  ADR 0001's "only the open gateways are checked".
