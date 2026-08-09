#!/usr/bin/env bash
#
# Deprecated. `arin service enable|disable|status|restart` is the command now.
#
# This script did the same job and had to be told where Arin.app was, because a shell
# script cannot ask. That argument is where every reported problem with it came from: the
# Homebrew line in the docs omitted it, so the agent was written against
# /Applications/Arin.app, which on a machine with both installs is a different build than
# the one that was asked for. A process can find its own bundle, so `arin service` cannot
# make that mistake, and it refuses to fight a nix-darwin managed agent rather than quietly
# winning.
#
# Kept because this path is written down in released Homebrew caveats and in docs people
# have already read. It forwards, and says what to type instead. Remove it a release after
# the caveats stop naming it.
#
# Usage, unchanged:
#   launch-agent.sh enable [/path/to/Arin.app]
#   launch-agent.sh disable
#   launch-agent.sh status

set -euo pipefail

action="${1:-status}"
app="${2:-}"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# The binary to forward to, which is also the one whose bundle becomes the agent. The named
# app first, then the bundle this script is installed in, then PATH. `cd` and `pwd` rather
# than realpath, so a Homebrew opt path stays the opt path: resolving it would reach the
# versioned keg and pin the agent to a version the next upgrade deletes.
if [ -n "$app" ]; then
	binary="$app/Contents/MacOS/arin"
elif [ -d "$here/../MacOS" ]; then
	binary="$(cd "$here/../MacOS" && pwd)/arin"
else
	binary="$(command -v arin || true)"
fi

if [ -z "$binary" ] || [ ! -x "$binary" ]; then
	echo "no arin binary found. Name the app: $(basename "$0") $action /path/to/Arin.app" >&2
	exit 1
fi

echo "$(basename "$0") is deprecated. This is now: arin service $action" >&2
exec "$binary" service "$action"
