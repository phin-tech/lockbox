set shell := ["bash", "-euo", "pipefail", "-c"]

default:
    @just --list

# Run the app from a bundle, which is what gives it its Dock icon
[positional-arguments]
run *args:
    "$(scripts/bundle-macos.sh debug)" "$@"

# Run the app in a forced appearance: `just run-as dark`
run-as appearance:
    PLAYPEN_APPEARANCE={{appearance}} "$(scripts/bundle-macos.sh debug)"

# Run the app with the element inspector (⌥⌘I): layout, styles and the source of whatever you pick
inspect:
    CARGO_FEATURES=devtools "$(scripts/bundle-macos.sh debug)"

# Run all tests, with cargo-nextest if it's installed (each test in a process of its own, and faster)
test:
    if command -v cargo-nextest >/dev/null; then cargo nextest run --workspace; else cargo test --workspace; fi

# Accept or reject changed snapshots (`insta::assert_snapshot!`), with cargo-insta
review:
    cargo insta review

# Licences, advisories and sources of every dependency (cargo-deny), and spelling (typos), each if it's installed
lint:
    if command -v cargo-deny >/dev/null; then cargo deny check; else echo "cargo-deny isn't installed: brew install cargo-deny"; fi
    if command -v typos >/dev/null; then typos; else echo "typos isn't installed: brew install typos-cli"; fi

# Save a rendered frame to target/shots/<name>.png, without showing a window: `PLAYPEN_KEYS="space space" just shot`.
# The settings are left alone; `PLAYPEN_SETTINGS=<file>` tries some, `PLAYPEN_WIDTH=600` a narrower window
shot name="app" appearance="light":
    @mkdir -p "${CARGO_TARGET_DIR:-target}/shots"
    PLAYPEN_APPEARANCE={{appearance}} PLAYPEN_SHOT="$(cd "${CARGO_TARGET_DIR:-target}/shots" && pwd)/{{name}}.png" \
        cargo run -p playpen --features screenshot

# Add a LiteLLM provider with a made-up key to the real OpenShell gateway, give it another, and delete it
providers-test:
    cargo test -p playpen-core --test providers -- --ignored

# Make a sandbox on the real OpenShell gateway with two folders, then add a third while it runs
folders-test:
    cargo test -p playpen-core --test folders -- --ignored

# Attach a throwaway read-only service to sandboxes on the real OpenShell gateway, then detach it
services-test:
    cargo test -p playpen-core --test services -- --ignored

# Copy a repository with uncommitted edits into a sandbox, change it there, and bring the work back as a branch
bring-back-test:
    cargo test -p playpen-core --test bring_back -- --ignored

# Make a sandbox, have it reach a host its policy doesn't allow, and approve the request that makes
requests-test:
    cargo test -p playpen-core --test requests -- --ignored

# Set up the MicroVM gateway, make a Claude Code sandbox on it, and remove it again; not over one already set up
microvm-test:
    cargo test -p playpen-core --test microvm -- --ignored

# Make a sandbox with a kit whose install.sh puts in jq and whose dotfiles are linked into its home
kit-setup-test:
    cargo test -p playpen-core --test kit_setup -- --ignored

# Make a Claude Code sandbox on the real OpenShell gateway and ask Claude Code something in it, with the provider named
openshell-test provider:
    PLAYPEN_TEST_PROVIDER={{provider}} cargo test -p playpen-core --test openshell -- --ignored

# Make a Codex sandbox on a ChatGPT provider Playpen signed in, and ask Codex something in it
codex-test provider:
    PLAYPEN_TEST_CHATGPT_PROVIDER={{provider}} cargo test -p playpen-core --test openshell codex -- --ignored

# Make a Pi, an oh-my-pi, a PiG and an OpenCode sandbox on an API-key provider, and ask each something; a LiteLLM one
# needs a model
pi-test provider model="":
    PLAYPEN_TEST_PI_PROVIDER={{provider}} PLAYPEN_TEST_PI_MODEL={{model}} cargo test -p playpen-core --test openshell pi_omp -- --ignored

# Make a Cursor sandbox on a Cursor provider, and ask its agent something
cursor-test provider:
    PLAYPEN_TEST_CURSOR_PROVIDER={{provider}} cargo test -p playpen-core --test openshell cursors -- --ignored

# Build Playpen.app and put it in /Applications, signed only for this Mac
install-local:
    scripts/install-local.sh

# Remove the copy in /Applications
uninstall-local:
    rm -rf "/Applications/Playpen.app"

# Record a Time Profiler trace of a release build with symbols, to target/profile.trace, and open it in Instruments
profile:
    rm -rf "${CARGO_TARGET_DIR:-target}/profile.trace"
    xcrun xctrace record --template 'Time Profiler' --output "${CARGO_TARGET_DIR:-target}/profile.trace" \
        --launch -- "$(scripts/bundle-macos.sh profiling)"
    open "${CARGO_TARGET_DIR:-target}/profile.trace"

# OrbStack's own Kubernetes can't run OpenShell's sandboxes (no `supplementalGroupsPolicy`); kind's containerd can
# Start a kind cluster in OrbStack's Docker for trying a Kubernetes gateway; does nothing if it's already up
kind-up name="openshell":
    command -v kind >/dev/null || { echo "kind isn't installed: brew install kind"; exit 1; }
    docker --context orbstack info >/dev/null 2>&1 || { echo "OrbStack's Docker isn't running: open -a OrbStack"; exit 1; }
    if kind get clusters | grep -qx "{{name}}"; then echo "kind-{{name}} is already up"; \
        else DOCKER_CONTEXT=orbstack kind create cluster --name "{{name}}" --wait 120s; fi
    kubectl --context "kind-{{name}}" get nodes

# Delete the kind cluster `kind-up` made
kind-down name="openshell":
    DOCKER_CONTEXT=orbstack kind delete cluster --name "{{name}}"

# Put an image from this Mac's Docker into the kind cluster, which can't pull from it: `just kind-load playpen-claude`
kind-load image name="openshell":
    DOCKER_CONTEXT=orbstack kind load docker-image "{{image}}" --name "{{name}}"

# Rebuild packaging/AppIcon.icns from packaging/logo.png
icon:
    uv run --quiet --with pillow packaging/make_icon.py

fmt:
    cargo fmt --all

# Formatting, clippy (with the screenshot feature too, which nothing else compiles) and every test
check:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo clippy -p playpen --all-targets --features screenshot -- -D warnings
    cargo test --workspace

# Run the CI in .ci.star on this machine, against what's committed (needs starci)
ci:
    starci run . --bare

# Remove build output. The installed copy stays
clean:
    cargo clean

# The release recipes: app-release, in release.just, which gpui-template keeps up to date
import 'release.just'
