# Changelog

All notable changes to this project are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Minors
ship biweekly, but nothing is tagged before 1.0: 0.x versions are cycle boundaries rather
than releases, so everything below stays under `[Unreleased]` until `v1.0.0`. 1.0 is the
protocol freeze, after which protocol changes are additive only.

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
- Invalidations now reach the client that owns them. The daemon has always worked out
  that a mark went away because content scrolled, a time to live ran out, or the user
  cleared the screen, and then had nowhere to send it: the socket answered requests and
  nothing else, so an agent could carry on describing something that was no longer on the
  screen. Each connection now subscribes, and a session is only ever told about its own
  marks, since learning that another client's annotation went away would leak that it
  existed. `--hold` prints them as they arrive, and the MCP tools carry them on the next
  result as `gone`, MCP having no way for a server to interrupt a model.
- `session_end` is answered with an `ack` rather than an `invalidated`. Every request now
  gets an ack or an error, which leaves `invalidated` to mean one thing only: something
  the client did not ask for. A reply that shared a type with a push could not be told
  apart from one.
- The menu bar reports what the daemon is holding and whether Screen Recording is
  granted, both refreshed each time the menu opens rather than fixed at startup, which is
  the one moment when nothing has happened and the permission is most likely missing. The
  permission line opens System Settings and is only enabled when there is something to
  fix.
- The daemon shuts down on `SIGTERM` and `SIGHUP` as well as Ctrl-C, and quitting from
  the menu bar goes through the same path. All four now unlink the socket on the way out;
  previously only Ctrl-C did, and `terminate:` from the menu ended the process without
  unwinding the daemon at all.
- Named positions on `point`, as a third target form beside coordinates and a query.
  `at` takes one of nine names, `top-left` through `bottom-right`, or a percentage pair
  like `50%,30%`, and the daemon resolves it against the display it was sent to, which is
  the whole reason it exists: a client that has not taken a screenshot cannot name a
  coordinate but can still say where it means. `--at` on the CLI, `at` on the MCP tool.
  Corners resolve a tenth of the way in rather than to the origin, since a mark at the
  literal corner is clipped by the edge. `"50,30"` without the signs is refused rather
  than read as percentages, because it is indistinguishable from the coordinates `x` and
  `y` take and would be wrong by a factor of the display size.
- The contrast picker scores where a mark puts ink rather than the region it was asked
  for. A highlight is an outline, so its interior is never painted and sampling the whole
  rectangle answered a question nobody asked; the four edges are now scored separately and
  the worst one decides, which is what catches a coloured band running under one edge. A
  freehand path is scored along its stroke, in four chunks of equal length, rather than
  over a bounding box that for a diagonal line is mostly pixels the stroke never touches.
  The number of parts is bounded on purpose: scoring every segment would be the worst-case
  statistic again, and that has no signal in it.
- Contrast adaptive annotation colour. The daemon samples the region it is about to draw
  over and keeps the usual amber unless amber genuinely cannot be seen there, at which
  point it picks from a small palette that never includes blue, since blue belongs to the
  orb. Scored against the median sample: a region of real interface contains something
  near black and something near white, so scoring the worst pixel gives every candidate
  about 1.0 and decides nothing. A colour a client named is never second-guessed, and a
  capture that fails falls back to the default rather than failing the request. Turn the
  whole thing off with `adaptive_color` to draw everything in the default and never
  capture except for scroll detection.
- Colour is resolved once, in the daemon, and reaches the renderer already decided. The
  macOS backend no longer parses hex or knows what the default is, so the two platforms
  still to come cannot drift from it.
- Time to live, per annotation. `ttl_ms` on `point`, `highlight`, `textbox`, and `draw`,
  `--ttl` in seconds on the CLI, and `ttl_seconds` on the MCP tools. The daemon sweeps on
  a timer rather than arming a timer per mark, since the alternative is a task per
  annotation that has to be cancelled whenever a clear, a scroll, or a session end gets
  there first. A client's own TTL wins over the configured default, and a zero is refused
  rather than drawn and swept in the same breath, because at that point it is a unit
  mistake more often than an intent. The plumbing existed since 0.1 and nothing called it.
- `arin-mcp`: the MCP server, over stdio, built on `rmcp`. Four tools, `point_at`,
  `highlight`, `annotate`, and `clear`, named after what an agent is trying to do rather
  than after the wire message underneath. It opens one session on startup and holds it, so
  a mark survives across turns of a conversation, and closing the client ends the session
  and takes the marks with it. Every tool reports back the display's size and scale, which
  is what an agent working from a screenshot needs in order to send logical points rather
  than pixels. A daemon refusal is passed through with its own message and wire code
  intact, since that is what tells a model how to phrase the next call.
- `DisplayId::DEFAULT`, the display a client fills in when the user named none. The wire
  contract is unchanged: every positioned message still carries an explicit `display_id`,
  and the daemon still never substitutes one.
- Caption rendering for `--label` on points and highlights. The protocol has always
  carried the field and the daemon has always stored it, but the macOS renderer dropped
  it, so `arin point 412 88 --label Save` acked and drew an unlabelled orb. A caption is
  now a dark pill sized to its own text, placed beside the orb or above the region.
  Placement has a preferred side and a fallback, so a mark near an edge puts its caption
  on the side with room rather than half off the display, and a label too long to be one
  truncates instead of running the width of the screen.

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
- The README is rewritten around what Arin now is rather than what it was mid-0.1. It
  gained an install line, the one-line MCP setup, the four tools, and a protocol summary,
  and lost a status section claiming that capture, the menu bar, the hotkey, and the orb's
  flight were all still to come. Every command and JSON example in it was replayed against
  a running daemon.
- The repository is `github.com/anistark/arin`, set once on the workspace and inherited by
  every crate. The copyright holder is named. Both were placeholders blocking the
  crates.io publish in 0.3.
- The phoenix logo is a new mark: a flat silhouette in a single blue with a soft glow,
  shipped as a transparent PNG. It replaces the gradient SVG pair, so the README header
  no longer animates. The orb is unchanged and stays the on screen primitive.
- Protocol coordinates are converted to AppKit's orientation in one tested function
  rather than by asking Core Animation to flip the panel's layer. `setGeometryFlipped`
  did not take on the overlay's content view, which drew every annotation at the wrong
  end of the screen.

### Fixed

- `LineReader::next_line` was not cancellation safe, which the socket server now depends
  on: it reads inside a `select!` against the invalidations it pushes, so the read future
  is dropped whenever an announcement wins the race. Bytes are consumed from the reader as
  they are seen and the buffer was cleared on entry, so a message split across two reads
  lost its first half and arrived truncated. It parsed as a schema error rather than as
  anything alarming, which is how it would have gone unnoticed. The buffer now survives
  between calls and is cleared once a line has been handed out.
- Excluding Arin's own windows from a capture was documented as not working, on a
  measurement taken before the frame geometry was right. Re-measured, it does work: a
  textbox covering a third of the display did not reach a colour picked for that same
  region afterwards. The note was wrong rather than the code.
- A downscaled capture reported the area it covered as its own pixel count over the
  display's backing scale, so a 512 wide frame of a 1512 point display claimed to cover
  256 points. Anything mapping a rect into that frame landed in the corner, which is where
  the contrast picker was sampling until this was found. A frame's `logical_size` is now
  the display's whatever resolution it was recorded at, and its `scale` is its own pixels
  per point, which is what `width == logical_size[0] * scale` always claimed.
- An expiring annotation did not count as the daemon changing the screen, so scroll
  detection compared a frame containing the mark against one without it and read the
  difference as the page moving. Every other annotation on that display went with it.
  Only reachable once something actually swept, which nothing did until now.
- A capture request made from inside ScreenCaptureKit's own completion handler could hang
  for the full 30 second timeout with no error. The two requests are now made in sequence
  from the calling thread, which turns a silent hang into an immediate and accurate
  failure.

### Known gaps

- Only one process can capture at a time. While the daemon runs, a second process asking
  ScreenCaptureKit for a screenshot has its request dropped without an error, so
  `arin capture` does not work alongside a live daemon. The daemon is unaffected, and
  `arin permissions` defers to it rather than reading the failure as a denied permission.
- Thin content still cannot decide a colour on its own, and this is the design rather
  than a limitation of the capture. A five point bar reaches about a fifth of the samples
  even at 512 pixels, and the median the picker scores by comes out at 9.12 against it at
  512, at 1024, and at full resolution alike. Capturing larger changes nothing and costs
  memory on every annotation. A minority of a region is meant to be outvoted; that is the
  property that made the picker usable when scoring the worst pixel turned out to give
  every candidate about 1.0.

  The case where a small share of the region is nonetheless most of the *ink* is now
  handled, by scoring the footprint rather than the region. What remains is a stroke that
  spends only a small fraction of its length over a different background, an eighth say,
  which is a minority of its own chunk and stays outvoted. It comes out legible where most
  of it is drawn and dim for the rest.

### Notes

0.1 is feature complete. Every annotation kind draws on macOS, capture is wired to
ScreenCaptureKit with a first run permission flow, and the marks can be cleared from the
menu bar or a global hotkey.

[Unreleased]: https://github.com/anistark/arin/commits/main
