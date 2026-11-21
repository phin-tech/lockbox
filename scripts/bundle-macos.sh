#!/usr/bin/env bash
# Builds Playpen.app, which carries the Dock icon and the bundle identifier, and prints the path of its binary.
# app-release builds its own for a release, from the same packaging/Info.plist and entitlements.
# Usage: scripts/bundle-macos.sh [debug|release|profiling], with CARGO_FEATURES=devtools for the inspector.
set -euo pipefail

profile=${1:-release}
root=$(cd "$(dirname "$0")/.." && pwd)
target=${CARGO_TARGET_DIR:-$root/target}

flag=()
[ "$profile" = debug ] || flag=(--profile "$profile")
if [ -n "${CARGO_FEATURES:-}" ]; then flag+=(--features "$CARGO_FEATURES"); fi
(cd "$root" && cargo build -p playpen ${flag[@]+"${flag[@]}"}) >&2

version=$(cd "$root" && cargo pkgid -p playpen | sed -E 's/.*(#|@)//')
build=$(sed -n 's/^build = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
app="$target/$profile/Playpen.app"
plist="$app/Contents/Info.plist"

mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$target/$profile/playpen" "$app/Contents/MacOS/Playpen"
cp "$root/packaging/AppIcon.icns" "$app/Contents/Resources/"
cp "$root/packaging/Info.plist" "$plist"
set_string() { plutil -replace "$1" -string "$2" "$plist"; }
set_string CFBundleDevelopmentRegion en
set_string CFBundleInfoDictionaryVersion 6.0
set_string CFBundleName "Playpen"
set_string CFBundleDisplayName "Playpen"
set_string CFBundleExecutable "Playpen"
set_string CFBundleIdentifier "tech.phin.playpen"
set_string CFBundleIconFile AppIcon
set_string CFBundlePackageType APPL
set_string CFBundleShortVersionString "$version"
set_string CFBundleVersion "$build"
codesign --force --sign - --entitlements "$root/packaging/app.entitlements" "$app" >&2

echo "$app/Contents/MacOS/Playpen"
