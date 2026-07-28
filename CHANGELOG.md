# Changelog

All notable changes to this project are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Minors
ship biweekly. 1.0 is the protocol freeze, after which protocol changes are additive
only.

## [Unreleased]

### Added

- `arin-protocol`: the 0.1 wire protocol as types plus validation, with no IO. Message
  envelope and version negotiation, logical geometry, anchors, opaque identifiers, and
  golden tests pinned to the spec examples.
- `arin-core`: the daemon. Unix socket server on a 0600 socket with a peer credential
  check and a 1MB payload cap, the session and annotation state machine, the `Renderer`,
  `Capture`, and `Resolver` seams, the scroll watcher, and a socket client shared by the
  CLI and the MCP server.
- `arin-cli`: the `arin` binary, with `daemon`, `point`, `highlight`, `annotate`, `draw`,
  `clear`, and `status`. `daemon --headless` runs the whole protocol with no renderer.
- `arin-resolve`: the resolver registry. No adapters yet, those land in 0.3.
- `arin-linux`, `arin-win`, `arin-mcp`: crate scaffolds carrying their documented scope.
- CI covering the two invariants the architecture rests on: core and the protocol build
  and test on Linux with no platform crate in the tree, and no input synthesis API is
  referenced anywhere in the source.
- A `justfile` for the common tasks, including a `ci` recipe that mirrors what CI runs.
- `arin-mac`: the overlay. One transparent, click through, non activating `NSPanel` per
  display, on all Spaces and above the menu bar, plus display enumeration from `NSScreen`
  and the orb built from three radial gradient layers. Points and highlights sent over
  the socket now appear on screen.
- `arin displays`, which lists the display ids to pass to `--display`. They are the ids
  macOS assigns rather than a count from one.
- `--hold` on `arin point` and `arin highlight`, which keeps the mark up until
  interrupted. Annotations live as long as the session that made them, so a one shot
  command otherwise clears its own mark on the way out.
- The clear affordance: a menu bar item and a global hotkey, `Cmd+Shift+K`. Either
  removes every annotation, whoever drew it. A session can only clear its own marks by
  design, so this is the one route that is the user's rather than an agent's.
- The orb's five state vocabulary, its flight, and its ember trail. Points now fly to
  their target along a bowed arc, squashing along the direction of travel and trailing
  sparks, then flare and settle into a slow pulse. Idle, thinking, travelling, pointing
  and ending each have their own pulse rate and ember density.
- ScreenCaptureKit capture on macOS, which is what makes scroll detection live. Frames
  come back at the display's physical resolution, so a Retina panel is compared at the
  detail it actually has. A denied Screen Recording permission is reported with what to
  do about it.
- The Screen Recording first run flow. Arin prompts for the permission at startup, and
  because the macOS prompt for this one only offers a route to System Settings rather
  than granting anything itself, it then watches for the switch to flip and says when
  capture goes live. A user who answered the prompt on an earlier run gets taken straight
  to the right Settings pane, since macOS will not ask a second time.
- `arin permissions`, which reports whether capture actually works rather than whether it
  is permitted. The two differ: macOS reports a grant immediately, but ScreenCaptureKit
  will not serve a process that was already running when the grant landed, and the only
  fix is a restart that nothing else tells you about.
- Text box and path rendering on macOS, and the `arin annotate` and `arin draw` commands
  that reach them. A text box is a rounded panel with a tinted border, sized in points so
  it stays legible on a Retina display. A path is stroked with round caps and joins, and
  takes an optional colour and width.

### Changed

- The daemon no longer owns the main thread on macOS. AppKit requires it, so the overlay
  runs its event loop there and the daemon moves to a worker thread. Every other platform
  is unchanged.
- The scroll watcher reports a capture failure once per display rather than on every
  tick, which was two log lines a second while capture was unimplemented.
- The scroll watcher runs its tick on a blocking thread. Capture is synchronous and the
  first call waits on the permission dialog, which is not something a runtime worker
  should be doing.
- Change detection compares a grid of sampled brightnesses rather than hashing the frame.
  An exact hash cannot tell an annotation appearing from the page moving underneath it,
  so the daemon used to clear the mark it had just been asked to draw. Comparing how much
  of the screen changed separates the two: a scroll moves most of what is on screen,
  while a mark is small and local.
- The scroll watcher only captures displays that have something drawn on them, and
  re-baselines around the daemon's own drawing.
- Protocol coordinates are converted to AppKit's orientation in one tested function
  rather than by asking Core Animation to flip the panel's layer. `setGeometryFlipped`
  did not take on the overlay's content view, which drew every annotation at the wrong
  end of the screen.

### Fixed

- A capture request made from inside ScreenCaptureKit's own completion handler could hang
  for the full 30 second timeout with no error. The two requests are now made in sequence
  from the calling thread, which turns a silent hang into an immediate and accurate
  failure.

### Known gaps

- Only one process can capture at a time. While the daemon runs, a second process asking
  ScreenCaptureKit for a screenshot has its request dropped without an error, so
  `arin capture` does not work alongside a live daemon. The daemon is unaffected, and
  `arin permissions` defers to it rather than reading the failure as a denied permission.
- A macOS capture contains Arin's own overlay. Both documented ways to exclude it were
  tried, by window and by application, and neither keeps it out of the frame. Change
  detection tolerates this rather than depending on the exclusion working, so it is not
  blocking, but a capture is not a clean shot of the screen underneath.

### Notes

0.1 is feature complete. Every annotation kind draws on macOS, capture is wired to
ScreenCaptureKit with a first run permission flow, and the marks can be cleared from the
menu bar or a global hotkey.

[Unreleased]: https://github.com/your-org/arin/compare/HEAD
