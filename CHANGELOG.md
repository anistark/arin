# Changelog

All notable changes to this project are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

**`0.2.0` is the first tagged release**, and the first build anyone outside can install.
The plan had been to tag nothing before 1.0, on the grounds that 0.x versions are cycle
boundaries rather than releases. That bent for a practical reason: a release pipeline
cannot be rehearsed without a tag, and running it for the first time on launch day is the
failure it exists to prevent.

Two things it is worth knowing before reading the versions below.

**The crate version is not the wire version.** `arin-protocol` and `arin` are at 0.3 while
the protocol they describe is still at 0.1, and the gap will keep widening. A Rust API change bumps the crate; a wire format
change bumps `PROTOCOL_VERSION`. The protocol is not frozen, and freezing it may wait for a
second renderer rather than landing at 1.0, since one implementation cannot prove a format
is a format.

**Versions are not cycle numbers either.** Development runs in numbered cycles that reach
0.7 and beyond in the plan; those never appear here. Only released versions do.

## [Unreleased]

## [0.3.0] - 2026-08-07

### Added

- Arin can tell you when a newer version has been released. Notify only: it downloads
  nothing and installs nothing, and that is a decision rather than a stage. Builds are
  unsigned, so fetching one would land you in front of Gatekeeper, and replacing a running
  app with no signature to check is a code execution path dressed as a convenience.

  Two ways in, and neither happens on its own:

  ```sh
  arin update                    # ask once. Running it is how you ask
  arin daemon --check-updates    # once a day, and the menu bar says so
  ```

  With the flag, the menu's name line becomes `Arin · 0.3.0 available` when there is one.
  The menu never does network work; it reads what the daily check already found. Without
  the flag nothing is spawned and no request is made, which matters for a daemon whose
  privacy claim is that grounding is the only thing that leaves the machine.

  Versions compare as numbers rather than as text, because `0.10.0` sorts before `0.9.0` as
  a string and after it as a version. A pre-release is never offered as an upgrade to
  somebody on a stable build, and a version that cannot be read reports nothing rather than
  nagging.

### Fixed

- **A second daemon could put launchd into a restart loop.** Finding the socket already
  served exited non-zero, and `KeepAlive` reads non-zero as a crash, so it respawned every
  ten seconds into an instance that could never bind. Watched filling the log on
  2026-08-06.

  It now exits zero, because this is not a failure: the socket has an owner and the user
  has what they wanted. The message also carries the whole error chain now, so the line
  says which path and who owns it rather than only `could not bind the socket`.
- **`arin permissions` answered for the wrong process.** It printed `screen recording is
  granted` while the daemon was logging that it could not capture.
  `CGPreflightScreenCaptureAccess` reports on whoever calls it, and macOS attributes a
  command run from a terminal to the terminal, so it was reporting the terminal's grant
  under Arin's name. It now says plainly that it cannot answer for the daemon, and points
  at the daemon's log, which is the only honest source.
- `packaging/macos/bundle.sh` warns when an installed `Arin.app` already exists. See
  **Known gaps**, since the underlying collision is not fixable here.

### Known gaps

- **Two Arin bundles compete for one Screen Recording grant.** macOS keys the permission to
  a bundle identifier, and for unsigned code it holds a separate record per binary behind
  that one name. A local `just bundle` and a Homebrew install both claim
  `com.anistark.arin`, System Settings shows a single "Arin" row for them, and toggling it
  updates whichever record it reaches while the other keeps asking. Cost an hour on
  2026-08-06 with the switch on and the daemon still unable to capture.

  `tccutil reset ScreenCapture com.anistark.arin` clears them all, after which the next
  start asks again. Giving development builds their own identifier would not help: the
  identifier is how the app recognises itself when Finder opens it, so a renamed build
  would print help instead of starting the daemon.

  The same thing makes the grant fail after every upgrade, since a fresh compile is a fresh
  binary. A Developer ID signature is what gives the app one identity across builds, and
  that is the reason signing has a release of its own.

## [0.2.1] - 2026-08-03

Nothing in the application changed. This exists to exercise the release path, which is
what the fix below is about.

### Fixed

- The release workflow's Homebrew tap job opened no pull request, and reported success
  while doing it. It copied the rendered formula into a clone of the tap and then asked
  `git diff --quiet` whether anything had changed. The tap had no `Formula/arin.rb` yet, so
  the copy was an untracked file, and `git diff` does not see untracked files. It concluded
  there was nothing to do and exited zero. `git commit -am` would have staged nothing for
  the same reason, so even reaching that line would not have helped.

  The formula is now staged before it is compared, and the comparison is `git diff --cached
  --quiet`. A job that does nothing is worse than one that fails, because nothing draws
  attention to it.
- The job also ran with an empty token. `HOMEBREW_TAP_TOKEN` was never set, and cloning a
  public repository works without one, so the failure would have surfaced several steps
  later as a push rejection that reads like a branch permission problem. It now checks for
  the secret first and says what it is and what scopes it needs.

## [0.2.0] - 2026-08-02

`arin-protocol` and `arin` were published to crates.io at `0.1.0` on 2026-07-30 with no tag
and no section here. Everything from then is folded into this release rather than
reconstructed after the fact, since no application was released at 0.1.0 and the version
could not be reused.

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
- `arin`: a facade crate re-exporting `arin-protocol`, so the wire types are reachable
  under the name the project is called. Published to crates.io alongside the protocol, and
  the only two crates in the workspace that are. It carries no binary: Arin is an
  application and is not distributed through Cargo, so `cargo install arin` reports that
  there is nothing to install, and the crate README points at the real install instead.
- `arin-mcp`: the MCP server, as a library rather than a second binary. `arin mcp` serves
  it on stdio, so an agent is pointed at Arin with one executable and one version:

  ```sh
  claude mcp add arin -- arin mcp
  ```

- `arin-resolve`: the resolver registry. No adapters yet, those land in 0.3.
- `arin-linux`, `arin-win`: crate scaffolds carrying their documented scope.
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
- Scroll tracking. Annotations move with content that scrolled instead of being thrown
  away by it. Movement is measured around each mark rather than across the display: the
  patch of screen surrounding it is reduced to brightness profiles, eight strips per axis
  so that a scroll in part of the patch is still visible, and the profiles from the
  previous tick are slid through the current frame to find where that content went. Marks
  translate by their own offset and are redrawn in place.

  The template and the window it is searched in are deliberately different sizes. Sliding a
  region against itself can only show half its own height of movement, which put a ceiling
  of a couple of hundred points on what could be followed, and ordinary scrolls are bigger
  than that: live, the winning offset was repeatedly the last one in range, and a mark was
  flung several hundred points off its content. The template stays tight around the mark so
  it measures that mark's own window rather than the desktop behind it, the window searched
  in the later frame is wider, and an offset landing on the end of the range is now refused
  rather than reported.

  Against a recorded corpus of 74 frame pairs this measures 11 of 12 movements correctly,
  refuses 1, invents none, and holds 61 of 62 still screens. It is not yet smooth in use:
  a mark follows one scroll and is invalidated by the next. See `plan/ROADMAP.md`.
  
  Measuring the whole display was tried first and is the obvious thing to build, which is
  why it is worth recording that it cannot work, and why `ARIN_RECORD` and
  `cargo run --example calibrate` now exist to settle this kind of question with
  measurements instead of argument. A scroll happens inside a window, so the
  menu bar, the dock, the desktop and every other window stay exactly where they were, and
  correlated across the whole screen the answer comes back as *nothing moved*. On a real
  laptop display, scrolling a text window: best offset zero, residual 4.6, with a fifth of
  the screen's samples changed. Globally true, and no use at all to a mark inside the
  window. Measuring locally also dissolves the case the display-wide version needed the
  content fingerprint to patch up after the fact: a mark on a toolbar beside a scrolling
  pane now simply measures no movement, which is correct rather than a special case.
- Grounding. `arin point "the Submit button"` and `arin highlight "the error message"`
  now work with no coordinates, as do `query` on the `point_at` and `highlight` MCP tools.
  The daemon captures the display, a resolver says where the thing is, and the mark goes
  there. This is what lets a client that cannot see the screen point at something on it.
- `arin-resolve`: the Claude adapter, grounding against a hosted model with the user's own
  API key. It sends a screenshot and a description and gets back a position, a bounding
  box, and a confidence, with the answer constrained to a schema. Deliberately not the
  computer use tool: that reports an action to take and carries no confidence, and
  confidence is what decides between a precise mark and a cautious one. Arin also does not
  actuate, so a request shaped like "click this" asks for something that will never happen.
  A model that cannot find the thing says so and nothing is drawn, because a mark on the
  wrong element is worse than no mark.
- `arin resolvers`, which lists what this build can ground with and whether each one
  leaves the machine. It builds each rather than describing it, so a resolver that is not
  going to work says why there rather than at first use.
- Display changes are now handled rather than ignored. The overlay rebuilds its panels
  when a display is attached, removed, or reconfigured, and the daemon drops the marks
  that went with it and redraws the ones that survived. Before this the panels and the
  display list were whatever they had been at startup: a monitor plugged in afterwards
  could never be drawn on, and marks on one that was unplugged sat in the daemon's state
  for the life of the session, invisible, unclearable from the menu bar, and keeping the
  scroll watcher asking for frames of a display that was not there.
- `display_change` is emitted at last. It has been a documented invalidation reason since
  0.1 with nothing in the daemon producing it. A mark on a display that goes away, or one
  left outside a display that shrank, now gets it.
- A dedicated display matrix, `crates/arin-core/tests/displays.rs`, running the same
  properties against six arrangements: one display at each scale, two matched, a Retina
  laptop beside a 1x external, three that differ in every respect, and a portrait panel
  beside a landscape one. Every one of them asserts that acks report that display's own
  scale and size, that named positions resolve against the right display, that the colour
  picker reads the frame for the display being marked, and that a scroll on one display
  leaves the others alone.
- `Capture::capture_detailed`, so one backend can serve two callers that want very
  different things. Scroll detection and the colour picker read coarse statistics from a
  thumbnail, which is why the daemon captures downscaled. A resolver has to read the
  interface, and a mark placed from a 512 pixel thumbnail is off by however much that
  thumbnail rounded.
- Content fingerprints, which fill in the `content_hash` the anchor has carried as null
  since 0.1. Each positioned mark records a 6x6 grid of average brightnesses from the
  region it was drawn over. After the daemon follows a movement it looks again at where the
  mark landed, and a mark now sitting on unrelated content is invalidated rather than left
  pointing at the wrong thing. The check runs even when the measurement says nothing moved,
  which is the case that needs it most: a region split between a still part and a scrolling
  one has two explanations, settles on zero, and would otherwise leave the mark sitting on
  content that had gone. Averages rather than single samples because the daemon compares
  512 pixel wide captures, where one pixel spans about three logical points and one sample
  of downscaled text swings further between two captures of the same content than it does
  between different content. Measured against the corpus, that change took the check from
  catching 2 left behind marks in 12 to catching 10. This is what covers the case a display-wide answer cannot:
  a page that scrolls under a toolbar that does not has one honest answer for most of the
  screen and a different one for the rest, and only the mark's own anchor knows which
  side of that line it is on.
- `arin-resolve`: the `local` adapter, which grounds against a model served on the same
  machine and is what removes both the API key and the one path by which a screenshot of
  the screen leaves the computer. `arin daemon --resolver local` and nothing about
  grounding touches the network.

  It speaks the OpenAI shaped `/v1/chat/completions` API rather than any one runtime's,
  because LM Studio, Ollama, vLLM, SGLang and llama.cpp's server all serve it and there is
  no standard for which of them a person should have installed. What that costs is a
  default port right for exactly one of them, so a resolver that cannot reach a server
  prints the others rather than a bare failure.

  `is_remote()` returning `false` is enforced rather than asserted. The endpoint is checked
  at construction and anything that is not `127.0.0.1`, `::1` or `localhost` is refused, so
  the resolver the consent story is built on cannot be pointed off the machine by editing an
  environment variable.

  Two answer shapes are accepted. A general vision model constrained to the schema reports
  a confidence and gets a precise mark. A UI TARS checkpoint answers with an action,
  `click(point='<point>512 384</point>')`, whatever the prompt asked for, because that is
  what it was trained to emit. That is accepted too, and it runs straight into the reason
  the computer use tool was rejected for grounding in 0.3: an action carries no confidence.
  So an unrated answer gets a constant below the threshold and draws a region. That is a
  placeholder for a measurement, and the eval set this cycle owes is what should replace it.
- `arin-resolve::grounding`, holding what both adapters do identically: the instructions,
  the schema, the range checks, and the one conversion out of the image the model saw into
  logical points. They differ in the API they reach through and nothing else, and two
  adapters quietly disagreeing about which corner a coordinate is measured from is the
  failure this prevents.
- `screenshot::encode_within`, so each adapter sets its own size limit. A hosted model has a
  hard ceiling and no marginal cost under it. A local one has no ceiling and nothing but
  marginal cost, since pixels are seconds of the machine's own GPU on a resolve somebody is
  watching an orb wait through.
- The annotation palette is configurable. `arin daemon --color '#FF2D95'` draws marks in
  magenta and keeps the built-in fallbacks, `--palette` replaces the set outright with the
  first entry preferred, and `--no-adaptive-color` stops the daemon looking at the screen
  to choose at all. `ARIN_COLOR` and `ARIN_PALETTE` do the same. Previously the palette was
  a constant with no way to reach it short of a rebuild.

  **Blue is refused rather than dropped.** The reservation rule was a property of a `const`
  covered by a test, and a configurable palette is a way to break it, so `Palette` checks
  every entry where it is built and rejects a blue one with the hue range and the reason.
  Accepting the palette and quietly removing the offending colour would leave someone
  watching marks come out amber with nothing anywhere to explain it.

  The daemon says at startup which palette it is using, but only when it is not the
  built-in one. A palette is set once and then forgotten, so "why are my marks green"
  months later wants an answer the daemon has and the person asking does not, while a line
  on every start saying marks are amber is noise.
- `arin diagnose`, a report to attach to a bug report. Arin collects no telemetry and never
  will, so there is nothing on our side to look at when something goes wrong. This is the
  replacement: version and target, the socket and whether anything is listening on it, the
  settings a daemon started here would use, every resolver and whether it can be built, the
  macOS version, the capture permission, the displays, and the environment variables Arin
  reads.

  It prints to the terminal rather than writing a file, because a report you have to open
  to see is one people attach unread, and this one is meant to be read. `--output` writes a
  file for the cases where that is easier.

  Secrets are reported as set or not set with a length, never quoted, and the rule is
  matched on the variable's name rather than on what the value looks like. A bundle exists
  to be pasted into a public issue, and a heuristic over values would eventually be wrong in
  the direction that costs someone a key.

  What it deliberately does not claim to know is the configuration of a *running* daemon.
  Nothing on the wire asks, so the section says it is reporting what a daemon started here
  right now would use, which is exactly the thing that differs in the case somebody is
  filing a bug about.
- **The security model, settled and implemented.** `plan/SECURITY.md` holds the argument.
  The finding is that Arin is a confused deputy for Screen Recording: Arin holds that grant
  and a client does not, so a client with no screen access of its own can ask where
  something is and read coordinates and a confidence back out of the ack. That is screen
  content laundered through Arin's permission, and on macOS it is a real privilege
  escalation.

  The answer is a capability split. **Drawing stays open to any peer that passes the uid
  check**, because a process running as the user could open its own always-on-top window and
  draw anyway, so gating it would cost every client a setup step and buy nothing. **Grounding
  is gated**, because it is the only capability Arin actually holds. `--grounding-consent`
  takes `ask`, the default, `always`, or `never`, and a refused query answers with a new
  `not_permitted` error code rather than `no_resolver`: one says grounding is not set up and
  the other says it was declined, and a client can act on exactly one of those.

  Permission is granted for a window of time rather than to a client, because there is
  nothing trustworthy to grant it to. `client_name` is self declared and binding to a peer
  pid is platform specific work in a crate that has no platform code, so that decision is
  deferred. A window also happens to be the only shape that works with the CLI, where every
  command is a new session: per-session approval would mean a prompt for every
  `arin point "the Submit button"`, and a control people turn off is worse than a weaker one
  they leave on. What it does not do is distinguish clients, and that is written down where
  it is implemented rather than only in the plan.

  `Approver` is a fourth platform seam beside `Renderer`, `Capture` and `Resolver`. Core
  decides when to ask and what an answer means. macOS decides what asking looks like, which
  is an `NSAlert` naming the client, quoting the query, and saying whether the screenshot
  leaves the machine, since granting a local model a look and a hosted one a copy are
  different decisions. The menu bar shows a live grant and revokes it, which is the promise
  the prompt makes.

  With `ask` and no approver wired, such as `--headless`, the answer is no rather than yes.
  A gate that opens when nobody is watching is not a gate, so an unattended daemon has to
  say `--grounding-consent always` out loud, and the daemon warns at startup when it does.

  Nothing here touches `session_start`, so the handshake is unchanged and `arin-protocol`
  needs no republish. That was the constraint the shape was chosen under.
- Two more things `plan/SECURITY.md` listed as owed. The egress warning now fires at the
  moment a screenshot goes out, not only at startup: "a remote resolver is configured" and
  "a picture of this display is leaving now" are different claims, and the second is the one
  worth finding in a log. And a session is capped at 256 annotations, so a runaway client
  cannot paper the screen. Per session rather than global, so one badly behaved client
  cannot refuse marks to a well behaved one sharing the daemon.
- **Arin.app**, an `LSUIElement` bundle, which is what makes Arin installable rather than
  merely buildable. A menu bar item and no Dock icon, built by `just bundle` into a
  universal binary with an icon generated from the same logo the README uses.

  The bundle is not cosmetic. macOS attributes the Screen Recording grant to a signed
  bundle identity rather than to a path, so a bare binary in `~/.cargo/bin` cannot hold
  that permission across an update. It also fixed `arin capture` failing beside a live
  daemon, without anybody writing code for it: ScreenCaptureKit was refusing one client
  identity twice, and the bundle is a second identity.
- A launch agent, so Arin can start at login. `just startup-enable`, or
  `launch-agent.sh enable` from inside an installed app. It points at the binary inside the
  bundle rather than the one on `PATH`, because an agent running some other build comes up
  unable to see the screen and unable to say why. Starting at login stays something the
  user turns on: a screen annotation daemon that added itself to login items unasked would
  be doing the thing people reasonably object to.
- A disk image, and a release workflow that builds, checks, signs, notarizes and publishes
  it. The workflow refuses to build when the tag and `[workspace.package].version` disagree,
  and refuses to publish a binary missing either architecture slice. **Signing and
  notarization are written and have never run**: they need an Apple Developer certificate,
  and the workflow skips them when the secrets are absent, which is how an unsigned build
  ships through the same path.
- A Homebrew formula, at `anistark/tools/arin`, and a `tap` job that opens a pull request
  against the tap on every release. It builds from source deliberately: Homebrew
  quarantines what it downloads, an unsigned app plus quarantine is a Gatekeeper block, and
  something compiled on the machine it runs on was never downloaded. So building from
  source is the only route that installs cleanly before there is a certificate, rather than
  the worse of two. A signed cask replaces it later.

### Changed

- **Bare `arin` prints help instead of failing**, because the subcommand is now optional,
  and `arin -d` runs the daemon in the foreground. It is built by parsing `arin daemon`
  rather than by constructing the variant, so the environment variables read for
  `--resolver` and the rest reach both spellings; a hand-built default would have honoured
  `ARIN_RESOLVER` under one and ignored it under the other. It takes no options of its own,
  and `arin -d clear` is refused rather than resolved by precedence.

  Opening Arin.app and typing `arin` are the same binary with the same empty argument list,
  and they now mean opposite things. They are told apart by `__CFBundleIdentifier`, which
  LaunchServices sets and a shell does not, matched against Arin's own identifier rather
  than merely tested for presence, with a terminal on stdin overriding it. Both guards fail
  towards printing help rather than towards starting a daemon nobody asked for.
- There is one orb for the whole system rather than one per display. Arin is a single
  agent, so it has a single presence: it points at one place at a time and moves between
  screens the way a mouse pointer does, and it belongs to the renderer host rather than to
  any one overlay window. Pointing at a second display used to leave the first display's
  orb behind, so two orbs sat on screen at once, saying there were two agents.

  A flight that crosses a screen boundary is planned once in the desktop's global space
  and cut into one segment per window it passes over, since a window cannot draw outside
  its own display. Each segment is drawn at its own display's backing scale, so the orb
  stays sharp crossing from a Retina laptop to a 1x external. The easing is carried by how
  the sampled positions are spaced along the arc rather than by a timing function per
  segment, so a flight drawn in three pieces still accelerates once instead of three
  times.

  Marks are unaffected and still stay on the screen they were drawn on, a point's caption
  among them. Nothing in `arin-core` or on the wire changed: a pointer position was never
  an annotation.
- A point redrawn as it follows scrolled content tracks rather than flying. The orb was
  setting off on a full flight on every tick of a scroll, when nothing about where
  attention should be had changed.
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
- A detected scroll no longer invalidates everything on the display as a matter of
  course. That is now the fallback, taken when no single movement accounts for what
  changed: a partial scroll, a window appearing, a page replaced outright. Three
  conditions have to hold before the daemon believes an offset instead. The profiles must
  line up at it, which is what stops a window scrolling inside a still screen from
  dragging every other mark along with it. No distant offset may score nearly as well,
  which is what stops evenly spaced lines of text from producing a confident answer one
  line pitch out. And a mark's own fingerprint must still match where it landed.
- The resolver registry holds builders rather than live resolvers. Configuration names
  one, and constructing any of them can fail, so a registry of instances would have to
  build every adapter it knows about in order to offer a choice between them. Someone who
  configured a local model would have needed an API key for the hosted one they did not
  ask for.
- Confidence now drives what gets drawn, which it could not before because nothing
  produced a confidence. The policy and its threshold have been in `arin-core` since 0.2
  with no resolver to feed them. A confident answer puts the orb on the target and an
  unsure one outlines the region instead.
- The capture the colour picker takes per positioned annotation now also records that
  annotation's fingerprint. It was hard to justify at around 100ms when it bought one
  thing; it buys two now, and the second is the only per-annotation evidence the daemon
  has that following a scroll put the mark somewhere sensible. Turning `adaptive_color`
  off still turns the capture off, so marks made that way are followed on the
  display-wide answer alone.
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

- `arin capture` works alongside a live daemon again, and nobody wrote code for it. The
  failure was ScreenCaptureKit dropping the second request, deallocating the completion
  handler without ever calling it, so it failed in about 200ms rather than hanging.

  It was described for a while as the machine allowing only one capturer, and that was
  wrong: Apple's own `screencapture` takes a frame quite happily while the daemon runs. The
  hypothesis that fit every measurement was narrower, that the daemon and the CLI were the
  same binary at the same path with no bundle, so ScreenCaptureKit could not tell the two
  clients apart. Confirmed twice, first by running the CLI from a different path, then by
  the app bundle, which is a second identity by construction. Two wrong explanations
  preceded the right one and the measurements are what separated them.
- The macOS renderer left a layer on screen when an annotation was drawn twice under the
  same id. `Renderer::draw` has always been documented as "draw or redraw", but the host
  only replaced its map entry, so the previous layer stayed in the tree with nothing left
  holding a reference that could remove it. Nothing redrew before now, so nothing had hit
  it. Following a scroll redraws constantly, and would have left a trail of every place
  a mark had ever been, only the newest of them clearable.
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

- **Signing and notarization have never run.** The release workflow imports a certificate,
  signs with the hardened runtime, notarizes, staples, and asks `spctl` what a user's
  machine would conclude. None of it has executed, because it needs an Apple Developer
  membership that does not exist yet. Until it does, the dmg on the release page is
  unsigned and macOS will refuse it on first open; `brew install anistark/tools/arin`
  builds locally and avoids that entirely, which is why it is the documented route.
- **`--resolver local` has never been run against a real model.** The adapter is covered
  end to end over real loopback HTTP with a real captured frame, against a stub server. Its
  two answer formats, both coordinate spaces and the whole conversion chain are exercised.
  What is unproven is whether a UI TARS class model is any good at this, and the confidence
  threshold is still a guess at 0.85 with no eval corpus behind it. Treat the resolver as
  experimental in the literal sense: nobody has watched it ground anything.
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
- Nobody has plugged a monitor in while the daemon was running. The panels rebuild and the
  daemon reconciles from an AppKit notification that no test can post, so both sides of
  that transition are covered and the transition itself has not been watched. The
  arrangements either side of it are covered against six layouts and on real hardware, a
  2x laptop with two 1x externals.
- A display that is reconfigured rather than removed loses its overlay and gets a new one,
  so every mark on it is redrawn from the daemon's state. That is correct and it is not
  free: a display whose parameters change repeatedly, as some do while waking, redraws
  everything on it each time. A panel that could be resized in place would avoid it, at
  the cost of repositioning every layer already on it against the new height.
- **Grounding accuracy is unmeasured.** The adapter is verified as far as a socket: the
  request shape, the required headers, the coordinate conversion, and every failure path
  are covered against a loopback server. None of that says whether it puts the orb on the
  right button, and it has never been run against the real API or a real screen. The
  effort level, the detail sent, and the confidence threshold are all starting points. The
  eval set is still owed, and the confidence threshold should not move off its 0.85 default
  until it exists.
- Grounding sends a screenshot of the whole display to a third party on every query. It is
  off unless named, an API key alone does not turn it on, and the daemon says so at
  startup, but that is the extent of the consent story. Whether consent belongs at the
  daemon, in the handshake, or per request is part of the security model that has to be
  settled before the protocol freezes.
- A resolve blocks the client that asked for it, for as long as the model takes. There is
  no progress on the wire while it runs, which is the open question about whether `ack`
  should stream. The orb sits in its thinking state, so the person watching sees
  something, and the agent does not.
- **Scroll tracking follows about half of real scrolls and invalidates the rest.**
  Measured against a recorded corpus of scrolls on a laptop display, judged against a full
  two dimensional comparison of each region: of eleven scrolls, six are followed correctly,
  four are refused and fall back to invalidating, and one is left where it is when it should
  have moved. No mark was placed anywhere wrong. Confirmed on a live screen, where a mark
  followed its content up by 82 points and other scrolls on the same page were refused.

  What refuses them is the two scorers disagreeing, and there is no cheap fix in hand: the
  obvious one, a gentler high-pass on the profile, was tried against the corpus and is
  measurably worse. Improving the rate means a better feature rather than a better
  threshold, and the corpus and its harness are checked in so that can be tried without a
  person sitting at a screen scrolling on request.
- A diagonal scroll is followed vertically and not horizontally when its horizontal
  component is not decisive on its own. The two axes are measured independently, and one
  of them naming a movement is allowed to account for the other being unreadable, because
  a vertical scroll genuinely scrambles the horizontal profile as new content arrives.
  Requiring both to agree refuses every real scroll, which is what a first attempt did.
  The mark ends up sideways of its target, and the fingerprint check is what catches it.
- Every threshold in the shift estimator is a starting number rather than a measured one.
  They separate the cases in the tests and on the screens they were written against, and
  they want a real corpus of scrolls behind them before anyone should trust the specific
  values. The one most likely to be wrong is how well the profiles must line up before an
  offset is believed, which is what decides how large a static region has to be before a
  partial scroll is refused.
- The fingerprint check passes on a bare majority of samples agreeing, which is a low bar
  set for an honest reason: the overlay is in the frame, so a mark recorded before it was
  drawn is compared against a capture containing it, and a text box covers its whole
  anchor. It reliably catches a mark stranded on unrelated content and is not being asked
  to tell a button from the same button one line lower.

### Notes

Feature complete on macOS, and now installable. Every annotation kind draws, capture is
wired to ScreenCaptureKit with a first run permission flow, marks can be cleared from the
menu bar or a global hotkey, grounding asks before it reads the screen, and the whole thing
ships as an app bundle with a Homebrew formula and a launch agent.

macOS only. Linux and Windows are planned and nothing of either is in this release; the
core and the protocol build and test on Linux with no platform crate in the tree, which is
what keeps that port cheap to pick up rather than evidence it works.

[Unreleased]: https://github.com/anistark/arin/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/anistark/arin/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/anistark/arin/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/anistark/arin/releases/tag/v0.2.0
