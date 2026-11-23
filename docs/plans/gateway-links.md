# Gateway links: a sandbox's service at a URL

Ports forwards a port inside the sandbox to this Mac's loopback. That works when the gateway is on this Mac, and only
while Playpen's forward process runs. OpenShell can also **expose** a port: the gateway serves it at a URL of its own, for
as long as the sandbox runs, with nothing running on the Mac
([sandboxes, "Expose Long Running
Services"](https://docs.nvidia.com/openshell/v0.1.9/how-it-works/sandboxes/overview)). For a sandbox on a Kubernetes or
remote gateway, that's the only good way to open its dev server.

Playpen calls it a **Gateway link** (`CONTEXT.md`). OpenShell calls it a service, which is Playpen's word for an outside
API reached with a provider's key.

## What the person sees

Under Ports, a second list, **Links**: each exposed port with its name and URL, an Open button, and Remove.
**Add a Link…** (`sandbox.link`) asks for the port and an optional name.

- `Authorization` headers are stripped by default. `bearer-passthrough` passes a caller's bearer token through to the
  service, for one that checks tokens itself. Offer it as a checkbox that says what it does, and leave it off.
- **What a link is on this Mac's gateway.** Tried on 0.1.9 on 2026-11-23. `service expose <sandbox> 8080` gave
  `http://default--<sandbox>.openshell.localhost:17670/`, which is `<workspace>--<sandbox>`, with a named service
  presumably adding its name. Opened from the Mac, plain `http://` answered 200 with no client certificate, and so did
  `https://`. The gateway listens on `127.0.0.1` only, and asking via the Mac's LAN address got nothing. So locally a link
  is a forward that needs no process and survives Playpen quitting, and any program on this Mac can open it without
  signing in. That's the same exposure as a forward, and the row should say "anyone on this Mac" so it isn't mistaken
  for something private.
- On a remote gateway, who can open the URL is up to that gateway's authentication. Try one before building the remote
  case.

## How it's built

- Core: `service expose <name> <port> [service] [--authorization-mode …]`, `service list -o json`, and `service delete`.
  The list is per gateway, so filter it by sandbox.
- Read when the sandbox is chosen and after a change, as forwards are.
