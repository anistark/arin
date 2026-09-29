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

# Asked of the rustc that cargo will call, rather than of rustup. Those are not always the
# same program: a rust from Homebrew or nix earlier on PATH than ~/.cargo/bin serves every
# build here while rustup goes on answering for a toolchain nothing is using. Believing
# rustup then means cross compiling to a target whose std is installed somewhere else, and
# the failure lands deep in a dependency rather than here:
#
#     error[E0463]: can't find crate for `core`
#     = note: the `x86_64-apple-darwin` target may not be installed
#
# A target's libdir existing is the thing that decides whether the build can happen, so ask
# for it directly and let whichever rustc is in charge answer.
targets=(aarch64-apple-darwin x86_64-apple-darwin)
available=()
for target in "${targets[@]}"; do
	libdir=$(rustc --print target-libdir --target "$target" 2>/dev/null || true)
	if [ -n "$libdir" ] && [ -d "$libdir" ]; then
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
# what Arin looks like. The .iconset layout is the documented one: five point sizes, each
# at 1x and 2x, named for the size they stand for.

iconset="$output_dir/AppIcon.iconset"
rm -rf "$iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
	sips -z "$size" "$size" assets/logo.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
	double=$((size * 2))
	sips -z "$double" "$double" assets/logo.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done

# Assembled here rather than by `iconutil -c icns`, which did it until 2026-09-16.
#
# Homebrew 7.0.0 (2026-09-13) refuses a build every mach lookup outside a short allowlist,
# and iconutil asks LaunchServices (com.apple.lsd.mapdb) whether the directory it was handed
# is an iconset before it reads a byte of it. Refused, it prints "Invalid Iconset" and exits
# 1, so from that release on every `brew install` stopped here, on every Mac, with nothing
# in this repository having changed. The resizes above are fine in there. `sips -s format
# icns` is not, for the same reason, so there is no tool to swap in.
#
# The format is small: a header, then one chunk per image, each a four letter tag, a
# length, and the PNG as it is. The tags are the ones iconutil writes for the same files,
# so Finder and the Dock see what they saw before. Two differences, neither visible: the 16
# and 32 point slots hold PNG (icp4, icp5) where iconutil writes raw ARGB (ic04, ic05),
# which macOS has taken since 10.7, and there is no `info` chunk, which is optional.

be32() {
	local n=$1 shift_by
	for shift_by in 24 16 8 0; do
		printf "\\$(printf '%03o' $(((n >> shift_by) & 255)))"
	done
}

# iconset file -> ICNS tag. 16@2x and 32 are the same 32 pixels, and both slots have to be
# filled, because macOS chooses by point size and scale rather than by pixel width.
slots=(
	icon_16x16.png:icp4 icon_16x16@2x.png:ic11
	icon_32x32.png:icp5 icon_32x32@2x.png:ic12
	icon_128x128.png:ic07 icon_128x128@2x.png:ic13
	icon_256x256.png:ic08 icon_256x256@2x.png:ic14
	icon_512x512.png:ic09 icon_512x512@2x.png:ic10
)

total=8
for slot in "${slots[@]}"; do
	total=$((total + 8 + $(stat -f%z "$iconset/${slot%%:*}")))
done
{
	printf 'icns'
	be32 "$total"
	for slot in "${slots[@]}"; do
		png="$iconset/${slot%%:*}"
		printf '%s' "${slot##*:}"
		be32 $((8 + $(stat -f%z "$png")))
		cat "$png"
	done
} >"$output_dir/AppIcon.icns"
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
# macOS keys Screen Recording to a bundle identifier, and for ad-hoc signed code it pins
# that one record to the exact binary it was granted to. System Settings shows it as a
# single "Arin" row, switched on, while every other build is refused. Cost somebody an hour
# on 2026-08-06, with the row switched on and the daemon still logging that it could not
# capture.
#
# A warning rather than a different identifier for dev builds: the identifier is how
# `Launch::detect` recognises its own bundle when Finder opens it, so a build that changed
# it would print help instead of starting the daemon, which is a worse trade.
installed="$(ls -d /opt/homebrew/opt/arin/Arin.app /Applications/Arin.app 2>/dev/null | head -1 || true)"
if [ -n "$installed" ]; then
	echo
	echo "    note: $installed also exists, and both claim com.anistark.arin."
	echo "    Screen Recording is granted per binary, so they compete. If it reads as"
	echo "    missing after you have granted it:"
	echo "        tccutil reset ScreenCapture com.anistark.arin"
	echo "    then grant it from the menu bar item of the one you meant to run."
fi
