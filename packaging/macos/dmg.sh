#!/usr/bin/env bash
#
# Package Arin.app into a disk image.
#
# The dmg is the direct download and it is also what the Homebrew cask fetches, so there is
# one artifact rather than one per install route. A cask can read a dmg directly, and a
# second zip would only be another thing to keep in step.
#
# Layout is the conventional one: the app, and a symlink to /Applications to drag it onto.
# No background image and no custom window geometry, which need AppleScript and a GUI
# session and are the usual reason a dmg build works locally and hangs in CI.
#
# Usage:
#   packaging/macos/dmg.sh [--app target/bundle/Arin.app] [--output target/bundle]

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

app="target/bundle/Arin.app"
output_dir="target/bundle"

while [ $# -gt 0 ]; do
	case "$1" in
	--app)
		app="$2"
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

if [ ! -d "$app" ]; then
	echo "no app at $app. Run packaging/macos/bundle.sh first." >&2
	exit 1
fi

version="$(/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$app/Contents/Info.plist")"
dmg="$output_dir/Arin-$version-universal.dmg"

# Staged in a scratch directory so the image contains exactly two entries and nothing the
# working tree happens to have lying around.
staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT

cp -R "$app" "$staging/"
ln -s /Applications "$staging/Applications"

rm -f "$dmg"
hdiutil create \
	-volname "Arin $version" \
	-srcfolder "$staging" \
	-ov \
	-format UDZO \
	"$dmg" >/dev/null

echo "$dmg"
shasum -a 256 "$dmg" | sed 's/^/sha256  /'
