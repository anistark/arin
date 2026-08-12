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

# `arin service` installs the launch agent now, and reads the template out of the binary
# rather than off disk. These two are what it replaced: a deprecated script that forwards to
# it, kept because released Homebrew caveats name this path, and the template itself, kept
# as the readable copy of what the agent is. They go together, and they go at the same time.
cp packaging/macos/launch-agent.sh "$contents/Resources/launch-agent.sh"
cp packaging/macos/com.anistark.arin.plist "$contents/Resources/com.anistark.arin.plist"
chmod +x "$contents/Resources/launch-agent.sh"

# Ancient, and still what Finder reads first to decide this is an application.
printf 'APPL????' >"$contents/PkgInfo"

# --- signing ----------------------------------------------------------------------------
#
# The hardened runtime is not optional for a release: notarization refuses a bundle without
# it, and the entitlements file exists to say how few holes are punched in it.
#
# Every bundle is signed, with or without a certificate. Skipping codesign entirely, which
# is what this did until 2026-08-11, does not produce an unsigned bundle. It produces a
# broken one: the linker ad-hoc signs the Mach-O on Apple silicon whatever anyone does, so
# the binary carries a signature that claims sealed resources while the bundle around it has
# no `_CodeSignature` at all. `codesign --verify` fails on it outright, its identifier is the
# linker's `arin-<hash>` rather than `com.anistark.arin`, and its Info.plist is not bound.
#
# TCC will not keep a Screen Recording grant against that. The daemon comes up reporting the
# permission missing on a machine where System Settings lists Arin with the switch on, and
# every start sends the user back to the same pane. Every Homebrew install built from source
# shipped in that state.
#
# So the ad-hoc branch is not a placeholder for the real thing. It buys nothing from
# Gatekeeper, which is what the formula used to say and is where the reasoning stopped, and
# it is the difference between having a bundle identity and having none.

if [ -n "$sign_identity" ]; then
	echo "==> signing as $sign_identity"
	codesign --force --deep --options runtime --timestamp \
		--entitlements packaging/macos/Arin.entitlements \
		--sign "$sign_identity" "$app"
else
	# No --timestamp, which needs Apple's timestamp server and has no meaning without a
	# certificate, and no --options runtime, which is a notarization requirement rather than
	# a local one.
	echo "==> ad-hoc signing. Gatekeeper will refuse this anywhere but here."
	echo "    The Screen Recording grant will hold for this build and stop at the next one."
	codesign --force --entitlements packaging/macos/Arin.entitlements --sign - "$app"
fi

# Both paths, because the failure this catches is one that only shows up as a permission
# that will not stick, a long way from here and with nothing pointing back.
codesign --verify --strict --verbose=2 "$app"

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
