#!/usr/bin/env bash
#
# Build Arin.app.
#
# The bundle is what makes Arin installable rather than merely buildable. It is why the
# app has no Dock icon, why Spotlight can find it, and why the Screen Recording grant
# survives an update: TCC remembers a bundle identifier and a signature, not a path in
# ~/.cargo/bin.
#
# Signing is optional here and required in CI. An unsigned bundle is fine for watching the
# thing work on the machine that built it, and is refused by Gatekeeper anywhere else, so
# the release workflow always passes an identity.
#
# Usage:
#   packaging/macos/bundle.sh [--sign <identity>] [--output <dir>]
#
# Environment:
#   ARIN_SIGN_IDENTITY   Same as --sign. The release workflow sets this.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

output_dir="target/bundle"
sign_identity="${ARIN_SIGN_IDENTITY:-}"

while [ $# -gt 0 ]; do
	case "$1" in
	--sign)
		sign_identity="$2"
		shift 2
		;;
	--output)
		output_dir="$2"
		shift 2
		;;
	*)
		echo "unknown argument: $1" >&2
		exit 2
		;;
	esac
done

# The manifest is the one place a version is written down. Reading it here rather than
# accepting one as an argument is what stops the bundle claiming a version the binary
# inside it does not report.
version="$(awk '/^\[workspace.package\]/{f=1} f&&/^version = /{gsub(/[",]/,"",$3); print $3; exit}' Cargo.toml)"
if [ -z "$version" ]; then
	echo "no version under [workspace.package] in Cargo.toml" >&2
	exit 1
fi

app="$output_dir/Arin.app"
contents="$app/Contents"

echo "==> Arin $version"

# --- the binary -------------------------------------------------------------------------
#
# Universal when both targets are installed, native when they are not. A release must be
# universal, so the workflow installs both and this refuses to guess: it says which slice
# it produced, and the workflow checks.

targets=(aarch64-apple-darwin x86_64-apple-darwin)
available=()
for target in "${targets[@]}"; do
	if rustup target list --installed 2>/dev/null | grep -qx "$target"; then
		available+=("$target")
	fi
done

if [ ${#available[@]} -eq 0 ]; then
	echo "==> building for the host only (no cross targets installed)"
	cargo build --release -p arin-cli
	binary="target/release/arin"
else
	slices=()
	for target in "${available[@]}"; do
		echo "==> building $target"
		cargo build --release -p arin-cli --target "$target"
		slices+=("target/$target/release/arin")
	done
	mkdir -p "$output_dir"
	binary="$output_dir/arin"
	lipo -create -output "$binary" "${slices[@]}"
fi

# --- the icon ---------------------------------------------------------------------------
#
# Generated from the same 1024px logo the README uses, so there is one source of truth for
# what Arin looks like. iconutil wants an .iconset directory of specific names and sizes.

iconset="$output_dir/AppIcon.iconset"
rm -rf "$iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
	sips -z "$size" "$size" assets/logo.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
	double=$((size * 2))
	sips -z "$double" "$double" assets/logo.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$output_dir/AppIcon.icns"
rm -rf "$iconset"

# --- assembly ---------------------------------------------------------------------------

rm -rf "$app"
mkdir -p "$contents/MacOS" "$contents/Resources"

cp "$binary" "$contents/MacOS/arin"
chmod +x "$contents/MacOS/arin"
mv "$output_dir/AppIcon.icns" "$contents/Resources/AppIcon.icns"

sed "s/@VERSION@/$version/g" packaging/macos/Info.plist >"$contents/Info.plist"

# The launch agent installer travels with the app, because the person who needs it has an
# installed app and not a clone. launch-agent.sh finds its template next to itself, so the
# two have to stay together.
cp packaging/macos/launch-agent.sh "$contents/Resources/launch-agent.sh"
cp packaging/macos/com.anistark.arin.plist "$contents/Resources/com.anistark.arin.plist"
chmod +x "$contents/Resources/launch-agent.sh"

# Ancient, and still what Finder reads first to decide this is an application.
printf 'APPL????' >"$contents/PkgInfo"

# --- signing ----------------------------------------------------------------------------
#
# The hardened runtime is not optional: notarization refuses a bundle without it, and the
# entitlements file exists to say how few holes are punched in it.

if [ -n "$sign_identity" ]; then
	echo "==> signing as $sign_identity"
	codesign --force --deep --options runtime --timestamp \
		--entitlements packaging/macos/Arin.entitlements \
		--sign "$sign_identity" "$app"
	codesign --verify --strict --verbose=2 "$app"
else
	echo "==> not signed. Gatekeeper will refuse this anywhere but here."
fi

echo "==> $app"
lipo -archs "$contents/MacOS/arin" | sed 's/^/    architectures: /'

# Two bundles carrying one identifier is a permission problem, not a tidiness one.
#
# macOS keys Screen Recording to a bundle identifier, and for unsigned code it holds a
# separate record per binary behind that one name. System Settings shows a single "Arin"
# row for all of them, so toggling it updates whichever record it happens to reach and the
# other keeps asking. Cost somebody an hour on 2026-08-06, with the row switched on and the
# daemon still logging that it could not capture.
#
# A warning rather than a different identifier for dev builds: the identifier is how
# `Launch::detect` recognises its own bundle when Finder opens it, so a build that changed
# it would print help instead of starting the daemon, which is a worse trade.
installed="$(ls -d /opt/homebrew/opt/arin/Arin.app /Applications/Arin.app 2>/dev/null | head -1 || true)"
if [ -n "$installed" ]; then
	echo
	echo "    note: $installed also exists, and both claim com.anistark.arin."
	echo "    Screen Recording is granted per binary, so they compete. If the daemon"
	echo "    keeps asking after you have granted it:"
	echo "        tccutil reset ScreenCapture com.anistark.arin"
	echo "    then start whichever one you actually meant to run."
fi
