#!/usr/bin/env bash
#
# Start Arin at login, or stop doing that.
#
# Separate from the cask on purpose. A Homebrew cask installs an app and does not manage
# user LaunchAgents, and `brew services` is a formula feature that a cask does not get. So
# starting at login is a decision the user makes after installing, which is the right shape
# anyway: a screen annotation daemon that added itself to login items without being asked
# would be doing something people reasonably object to.
#
# Usage:
#   packaging/macos/launch-agent.sh enable [/Applications/Arin.app]
#   packaging/macos/launch-agent.sh disable
#   packaging/macos/launch-agent.sh status

set -euo pipefail

label="com.anistark.arin"
plist="$HOME/Library/LaunchAgents/$label.plist"
log_dir="$HOME/Library/Logs/Arin"
template="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$label.plist"

action="${1:-status}"
app="${2:-/Applications/Arin.app}"

case "$action" in
enable)
	binary="$app/Contents/MacOS/arin"
	if [ ! -x "$binary" ]; then
		echo "no Arin.app at $app" >&2
		echo "Install it first, or pass the path: launch-agent.sh enable /path/to/Arin.app" >&2
		exit 1
	fi

	# Resolve to an absolute real path. launchd does not expand ~ and will not search PATH,
	# and a relative path here fails at login with nothing to read about why.
	binary="$(cd "$(dirname "$binary")" && pwd)/$(basename "$binary")"

	mkdir -p "$(dirname "$plist")" "$log_dir"
	sed -e "s|@ARIN_BINARY@|$binary|g" -e "s|@LOG_DIR@|$log_dir|g" "$template" >"$plist"
	plutil -lint "$plist" >/dev/null

	# bootout first so `enable` is repeatable: re-running it after moving the app should
	# replace the agent rather than fail on one already loaded.
	launchctl bootout "gui/$UID/$label" 2>/dev/null || true
	launchctl bootstrap "gui/$UID" "$plist"
	launchctl enable "gui/$UID/$label"

	echo "Arin starts at login, from $binary"
	echo "Logs: $log_dir/arin.log"
	echo
	echo "The Screen Recording grant belongs to this bundle. If you move or replace the"
	echo "app, run this again so the agent points at where it actually is."
	;;

disable)
	launchctl bootout "gui/$UID/$label" 2>/dev/null || true
	rm -f "$plist"
	echo "Arin no longer starts at login. The app itself is untouched."
	;;

status)
	if launchctl print "gui/$UID/$label" >/dev/null 2>&1; then
		echo "enabled"
		launchctl print "gui/$UID/$label" | grep -E "^\s+(state|program|last exit)" || true
	else
		echo "not enabled. Run: $(basename "$0") enable"
	fi
	;;

*)
	echo "usage: $(basename "$0") enable [/path/to/Arin.app] | disable | status" >&2
	exit 2
	;;
esac
