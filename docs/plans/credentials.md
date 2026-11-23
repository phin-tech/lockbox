# Credential trouble: say when a key failed, and what to do

Playpen already replaces a provider's key (`provider update`) and renews a ChatGPT sign-in (`provider refresh`). What it
can't do is notice that a key stopped working. Inside the sandbox, the agent gets an HTTP 502 carrying an
`x-openshell-request-id`, and the cause is hidden.

OpenShell records the latest failure on the sandbox: `sandbox get <name> -o json` has `last_token_grant_failure`. It
gives the host and port, a failure code, and a recovery action of `reauthorize`, `fix_configuration`, `retry` or
`investigate` ([sandboxes, "Read Token Grant
Failures"](https://docs.nvidia.com/openshell/v0.1.9/how-it-works/sandboxes/overview)). It keeps one failure, not a
history, and a successful grant to the same place clears it. The field is present, and `null`, on this Mac's 0.1.9. For a
tool server over HTTP, `endpoint_statuses` also reports `CredentialUnavailable` as its last result. Check the exact
field names on a real failure before building, since none has been seen here.

## What the person sees

A banner on the chosen sandbox, naming the provider and what to do:

- `reauthorize`: "Claude's sign-in has run out." with **Sign In Again…**, which goes to the provider in Settings ▸
  Providers, where replacing a key or signing in again already is.
- `fix_configuration`: the provider and OpenShell's message, with **Show Provider**.
- `retry`: "OpenShell couldn't reach the provider just now," which clears itself on the next good grant.
- `investigate`: OpenShell's message as it is.

The sidebar marks the sandbox, as it marks one with requests waiting.

## How it's built

- Core: `parse_sandboxes` reads `sandbox list -o json`, and `last_token_grant_failure` is in each sandbox there (checked
  on 0.1.9). So the listing Playpen already makes every few seconds carries it, and no extra call is needed. The codes
  are `provider_authorization_required`, `workload_identity_rejected`, `configuration_error`, `timeout` and
  `unavailable` ([profiles, "Token Grant
  Failures"](https://docs.nvidia.com/openshell/v0.1.9/how-it-works/providers/profiles)).
- No new Command. The banner's buttons go to Commands that already exist (`go.providers`).

## Also: refresh status

`provider refresh status` reports when each renewed credential was last refreshed and whether that worked. Settings ▸
Providers could show "renewed 2 hours ago" or "renewal failed" beside a ChatGPT provider. That's small and can come with
the banner.
