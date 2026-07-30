# AGENTS.md

Context file for coding agents working on Arin. Read this before touching anything.
Keep it current: if you change architecture, protocol, or a decision recorded here, update this file in the same commit.

---

## What Arin is

Arin is a local daemon that draws on the user's screen. Any AI agent can drive it over a small JSON protocol or through MCP. It points at things, highlights regions, writes explanatory text boxes, and draws freehand paths. It is the visual layer a teaching agent uses when it wants to say "look at this part".

Name: Arin, from Annotation Rendering INterface.

**What Arin is not, and must never become:**

| Not | Why |
|---|---|
| An input actuator | No synthetic clicks, keystrokes, scrolls, or typing. Ever. This is the product boundary and the trust story. Anything that calls CGEventPost, SendInput, or uinput does not belong in this repo. |
| A network service | The daemon binds a Unix domain socket only. It never opens a listening network port. |
| An AI | Arin holds no model, no prompt, no reasoning. Intelligence lives in the client. |
| A telemetry collector | Zero analytics, zero phone home, zero accounts. |

If a task seems to require breaking one of these, stop and ask. Do not work around them.

CI enforces the first one with a grep for input-synthesis APIs. If that check fires on something you wrote, the feature belongs in a different project.

---

## Architecture

```
CLIENTS (not ours)
Claude Code, Cursor, Cline, custom agents, arin CLI
        |
        |  MCP (stdio)          JSON lines over Unix socket
        v                                v
  +-----------+                   +--------------+
  | arin-mcp  | ----------------> |  arin-core   |
  +-----------+                   |              |
                                  |  protocol    |  auth, schema, sessions
                                  |  engine      |  annotation state, anchors
                                  |  traits      |  Renderer, Capture, Resolver
                                  +------+-------+
                                         |
                     +-------------------+------------------+
                     v                                      v
              +-------------+                       +--------------+
              |  arin-mac   |                       |  arin-linux  |  (0.4)
              |  objc2      |                       |  wgpu        |
              |  NSPanel+CA |                       |  layer shell |
              |  SCKit      |                       |  portal      |
              +-------------+                       +--------------+
```

### Crate map

| Crate | Contents | Platform code | State |
|---|---|---|---|
| `arin-protocol` | Message types, schema validation, version negotiation. Pure types, no IO. | no | done for 0.1 |
| `arin-core` | Daemon. Socket server, auth, session and annotation state machine, anchor model, scroll invalidation loop. Depends on traits only. | no | done for 0.1 |
| `arin-resolve` | `Resolver` registry and adapters. The only crate that reaches the network. | no | registry plus the Claude adapter |
| `arin-mac` | `Renderer` and `Capture` impls. NSPanel, Core Animation, ScreenCaptureKit via objc2. | macOS | done for 0.1 |
| `arin-linux` | Renderer via wgpu on wlr layer shell. Capture via xdg desktop portal. 0.4. | Linux | empty |
| `arin-win` | Layered window renderer, DXGI capture. 0.6. | Windows | empty |
| `arin-mcp` | MCP server. Translates MCP tool calls into socket messages. A library, served by `arin mcp`. 0.2. | no | four tools over stdio |
| `arin` | Facade library published to crates.io. Re-exports `arin-protocol` under the plain name. | no | published |
| `arin-cli` | The `arin` binary, the only one in the workspace: daemon control, MCP, debug commands, scripting client. | no | working |

The `Renderer`, `Capture`, and `Resolver` traits all live in `arin-core`, matching the diagram. `arin-resolve` holds the registry and the adapters that implement `Resolver`, not the trait itself.

### Hard dependency rules

1. `arin-core` and `arin-protocol` must compile and pass tests on Linux CI with no platform crate in the tree. If you add a Mac dependency to core, CI breaks and that is intended.
2. Platform crates depend on core. Core never depends on a platform crate. Wire concrete impls in the binary only.
3. `arin-protocol` has no IO and no async runtime. It is types and validation.

### Coordinates

**Every coordinate in the protocol is a logical point paired with an explicit `display_id`.** Never physical pixels. Physical conversion happens inside platform crates and nowhere else.

This is the single largest source of bugs in this class of software. Retina screenshots come back at 2x while the overlay draws in points. Mixed DPI multi monitor setups compound it. If you find yourself dividing by a scale factor outside a platform crate, something is wrong.

---

## Build and test

```
cargo test --workspace          # everything, headless, no display needed
cargo clippy --workspace --all-targets
cargo fmt --all
```

To exercise the daemon without a renderer:

```
cargo run --bin arin -- --socket /tmp/a.sock daemon --headless
cargo run --bin arin -- --socket /tmp/a.sock point 412 88 --display 1 --label Save
```

The whole state machine is testable with no display: platform behaviour arrives through traits, and the integration tests wire up fakes. Rendering gets screenshot diffs later. Grounding accuracy gets a proper eval set when the resolver lands in 0.3.

Internal planning notes live in `plan/`, which is gitignored. It holds the roadmap, the wire protocol draft, and the brand spec. If you need to know what ships when, or what the protocol is supposed to say, look there.

---

## Protocol

JSON lines over the Unix socket. Every message carries a `v` field. The wire protocol is the public contract, and it is SemVer'd.

Client to daemon:

```
session_start  {client_name}              -> {session_id}
point          {x, y, display_id, label?} raw coordinates, client did its own grounding
point          {query, display_id}        natural language target, needs resolver (0.3+)
highlight      {rect | query, display_id, label?}
textbox        {rect | anchor, text}      display only, never an input widget
draw           {path: [pts], style?}
clear          {annotation_id | all}
session_end
```

Daemon to client:

```
ack          {annotation_id | session_id, resolved_coords?, confidence?, display?}
invalidated  {reason: scroll | display_change | session_end | ttl}
error        {code, msg}
```

Every annotation carries an anchor descriptor `{screen_rect, display_id, content_hash?}`. From 0.3 all three are used: `content_hash` holds a coarse record of what the mark was drawn over, and the daemon re-reads it after following a scroll to check the mark landed on the same content. It is opaque and clients must not construct or interpret one.

Marks are moved silently. A scroll that the daemon can follow produces no wire traffic at all, because nothing the client asked for has stopped being true. Only a mark that could not be followed is announced, with `invalidated`.

Unknown fields are ignored and unknown message types return an error rather than closing the connection, so a client from a future minor version keeps working.

---

## Rendering

**Only one visual primitive is implemented in code: the orb.** Three concentric radial gradients plus a particle emitter.

**And there is only one of it, for the whole system.** The orb is Arin's pointer, not a mark. It has a single existence however many displays are attached, it draws attention to one place at a time, and it moves between screens the way a mouse pointer does. It belongs to the renderer host rather than to any one overlay window, and a flight that crosses a screen boundary is cut into one segment per window so it reads as one continuous movement. An orb per display would mean pointing at a second screen left the first one behind, and two orbs on screen say there are two agents.

Marks are the opposite: a highlight, a text box, a path, and a point's caption stay on the screen they were drawn on. So a second `point` moving the orb off the first is correct, and leaves no residue behind it. Nothing about this is in the protocol, because a pointer position is not an annotation.

The phoenix logo is a static brand asset. It is never rendered by the daemon. Do not add bird geometry to any renderer.

Orb state vocabulary. The client never requests these. They follow from daemon state.

| State | Rendering |
|---|---|
| Idle | Slow pulse, sparse embers, parked |
| Thinking | Faster pulse, denser embers, stationary. Active while a resolve or stream is in flight. |
| Traveling | Stretch along velocity vector, trail particles spawn along the bezier arc |
| Pointing | Settled at target, brief bright flare, embers calm |
| Ending | Dims to faint core, embers stop, fade out |

A circle has no facing, so travel reads through stretch, never rotation.

### Rendering rules

1. Glow requires radial falloff. On macOS use CAGradientLayer or a layer with a blur. In wgpu it is a short fragment shader. Do not try to fake it with stacked flat circles.
2. The overlay is 100 percent click through. There are no buttons in it. Clear is a menu bar item and a global hotkey.
3. Below 20 logical points, embers stop spawning and the halo tightens. The menu bar icon is the same primitive with features disabled, not a separate asset.
4. macOS menu bar needs a template image, monochrome, system tinted. Use the template asset, not the colour orb: a blue orb in the menu bar looks wrong in dark mode.

### Color

| Token | Hex | Use |
|---|---|---|
| deep | `#1E3A8A` | halo, outer ring |
| mid | `#3B82F6` | ring |
| core | `#A9DCFF` | inner core |
| spark | `#7FE3FF` | embers |
| annotation default | `#FFB020` | marks, amber |
| annotation fallback | magenta family | when content is already warm |

**Reservation rule: blue belongs to Arin.** The contrast picker chooses annotation colors per target region by sampling luminance, and it must exclude the blue family entirely. Without that exclusion a mark on a dark screen visually merges with the orb.

---

## Decisions already made

Do not relitigate these without asking.

| Area | Decision |
|---|---|
| Language | Rust core plus native platform crates. Chosen for the portable 80 percent, accepting a slower Mac layer via objc2. |
| Platform order | macOS, then Linux KDE and wlroots, then Windows. GNOME is out of scope: it does not support layer shell. |
| Teaching mode | Freeze frame. Capture once, annotate, valid until scroll. |
| Scroll detection | Screenshot diff on a 500ms tick during active sessions only. Keeps permissions at Screen Recording alone, no Accessibility TCC. |
| Scroll response | Follow the content where the movement can be measured, invalidate where it cannot. Never guess: a mark that vanishes is a failure the client can see, and a mark confidently pointing at the wrong thing is one it cannot. |
| Annotation lifetime | Session scoped. Clear 5 seconds after `session_end` or socket disconnect. |
| Clear affordance | Menu bar item plus global hotkey. No overlay button. |
| Grounding | CUA class models only. Raw coordinates from the client in 0.1. Resolver plugin registry from 0.3. |
| Grounding shape | Ask a vision model where something is and constrain the answer to a schema, rather than handing it the computer use tool and reading the coordinate out of a click it wants to make. A tool call carries no confidence, and Arin does not actuate, so a request shaped like "click this" asks for something that will never happen. |
| Resolver consent | Off unless named. An API key in the environment is not consent, so no adapter is selected by inference, and a daemon told to use a remote one says so at startup. Provisional until the security model is settled. |
| Displays changing | Panels are rebuilt on the platform's own screen change notification, and the daemon reconciles against the new arrangement: marks on a display that went, or now outside one that shrank, are dropped with `display_change`, and what survives is redrawn. A renderer cannot repopulate an overlay it just rebuilt, because the annotations live in core and it has never been told what they are. |
| One orb | One for the whole system, not one per display. Arin is a single agent, so it has a single presence and points at one thing at a time. It is re-parented between overlay windows rather than existing once per screen, so "there is one orb" is a fact about the layer tree instead of a discipline every renderer has to keep. A platform renderer that gives each display its own orb is wrong, and it is the shape the macOS one had until 0.4. |
| Crossing a screen | The flight is computed once in the desktop's global space and cut into one segment per window it crosses, because a window cannot draw outside its own display. Each segment is drawn by its own window at its own backing scale, which is what keeps the orb sharp on a mixed DPI arrangement. A single window spanning every display would make the geometry trivial and has one backing scale for all of them, so the orb would render soft on whichever display did not win. |
| Pacing a flight | The easing is carried by how the sampled positions are spaced along the arc, not by a timing function. Core Animation gives each adjacent pair of samples an equal slice of time, so spacing them is what sets the speed, and a flight drawn in three segments still accelerates once. A timing function per segment would ease within each one and the orb would surge at every screen boundary. |
| Who captures | The daemon, never the adapter. A resolver declares how much detail it needs through `Resolver::detail` and is handed a frame, so an adapter never touches the screen and a fake one in a test never has to. |
| Textboxes | Display only through all of 0.x. No input widgets. |
| Sequencing | Brain side. The daemon has no concept of step 2 of 7. |
| Audio | Never in the daemon. TTS belongs to the client. |
| Telemetry | None. |
| Publishing | Two crates on crates.io and no more: `arin-protocol`, the wire format, and `arin`, a facade re-exporting it so the plain name is taken and means something. Everything else carries `publish = false` permanently rather than provisionally. Publishing `arin-core` and the platform crates would put a semver obligation on the daemon's internals, in exchange for a shorter command than `cargo install --git` already provides. Revisit only with evidence that someone wants it. |
| Distribution | Nothing ships to a user before 0.5. Source only until then: `cargo install --path crates/arin-cli`, or `cargo install --git`. From 0.5 the GitHub releases, the signed and notarized dmg, the brew tap and the distro packages land together, with Sparkle carrying updates on macOS. |
| `cargo install arin` | Does not work, on purpose. `arin` is a library, so Cargo answers that there is nothing to install. That error is the first thing anyone guessing the command will see, so the crate README leads with `brew install --cask arin` and says outright that the application is not distributed through Cargo. Building from source is `cargo install --git https://github.com/anistark/arin arin-cli`. |
| One binary | `arin` and nothing else. MCP is `arin mcp` rather than a second executable, because an agent's MCP config is written once and outlives several updates: two binaries means two paths to keep on `PATH` and two versions to keep in step, and a stale one fails in ways that look like Arin being broken. Crates stay split as they are. It is the shipped executables that are collapsed, not the code. |
| Running the daemon | `arin -d` starts it, `arin daemon` is the long spelling, and both run in the foreground. Backgrounding is the shell's job or launchd's, not a hand rolled daemonizer: Screen Recording permission is bound to binary identity and a forked child is a good way to lose it quietly. Starting at login is a launchd agent. `-d` is a top level flag and never a modifier on `point` or `highlight`, which connect, send, and exit. |
| Launching from the GUI | An app bundle with `LSUIElement` set, wrapping the same binary with the daemon flag. Menu bar only, no Dock icon. This is also what makes the permission grant stick, which is why it lands with signing rather than before it. |

---

## Still open

Do not assume answers. Ask before building past these.

1. **Security model.** Baseline is a 0600 socket in a directory only its owner can traverse, a peer credential check, strict schema validation, and a 1MB payload cap. All of that is implemented. The full threat model, client authentication, annotation provenance, and resolver egress consent are unresolved. This must be settled before the protocol freezes at 1.0, because auth shape affects the `session_start` handshake.
2. Whether `ack` should stream progress for slow resolves, or clients should poll.

---

## Conventions

- Biweekly 0.x cycles. SemVer. Nothing is tagged before `v1.0.0`, which is the repo's first tag: 0.x versions are cycle boundaries, not releases, and the 0.x line ships from `main`.
- Protocol changes are additive within a major version. Reserved fields exist so this stays true.
- Tests: golden JSON tests for protocol and core, runnable headless with no display.
- Name is kept shallow in code. Binary name and crate prefix reference a single const. Socket path is config driven. A rename should be a day of work, not a migration.
- Errors map to exactly one wire error code, so a client never has to parse message text to know what happened.

## Code

- **Always check for existing libraries and tools that solve something before jumping to write an entire module for it.** Search crates.io first, prefer a well-maintained dependency over a homegrown module. Justify any hand-rolled implementation in the PR or commit description.
- **If something can be modularised or re-used, do that.** Shared logic goes in the appropriate crate, not copy-pasted across crates. Respect crate boundaries: modularity here means crate interfaces, not utils dumping grounds.
- **Avoid over-commenting.** Only useful doc-strings and `TODO`s and `NOTE`s as needed. No comments that narrate what the next line does, restate the diff, or justify a change to a reviewer.
- **Keep section comments plain.** `// fakes`, not `// --- fakes ---------------`. No ASCII rules, box drawing, or padding to a column.
- **Follow good variable and function naming conventions.** Standard Rust style: `snake_case` items, `CamelCase` types, names that say what a thing is or does, with no abbreviations that need decoding.

## Git

- **Never commit unless explicitly asked. Always use the `/commit-msg` skill to commit and stick to its instructions.**
- **Follow open source branch naming conventions, and open a branch when starting work on a module or feature.** `feat/<topic>`, `fix/<topic>`, `docs/<topic>`, `refactor/<topic>`, `chore/<topic>`, for example `feat/mac-overlay-panel`. Never work directly on `main`.
- **Keep the workspace [CHANGELOG.md](CHANGELOG.md) updated.** Follow [SemVer](https://semver.org) and [Keep a Changelog](https://keepachangelog.com) conventions: notable changes land under `[Unreleased]` in the same branch as the change. Since nothing is tagged before 1.0, that section keeps accumulating across the whole 0.x line and rolls into a `[1.0.0]` heading at the freeze. 1.0 is the protocol freeze, after which protocol changes are additive only.

## Docs

- **`docs/` holds public-facing docs only**: anything a user of the repo should read, such as the wire protocol once it is published, or an operating guide. **Everything else, meaning planning, drafts, research, and internal notes, goes in `plan/`**, which is gitignored and not part of the published repo. Never create internal scratch docs inside the repo proper.

## Writing

- **No em-dashes or semicolons in prose.** Applies to documentation, comments, commit messages, and any other written content. Use a full stop, a comma, or a colon instead. Rewriting the sentence is usually better than substituting punctuation.
- This is about prose only. Rust statement semicolons are syntax, and a semicolon inside a code sample or an identifier is not prose.

## Gotchas

- **Never run `xcodebuild` from a terminal for the Mac app.** It invalidates TCC permissions and you will spend an hour wondering why Screen Recording stopped working. Build through Xcode.
- Grounding accuracy for CUA models sits around 85 to 95 percent. Confidence drives the render: high confidence gets an arrow, low confidence gets a region highlight. A slightly large highlight looks intentional. A confident arrow pointing at the wrong button looks broken.
- Unix socket paths max out around 104 bytes including the terminator. Long temp directories blow through that, and the raw OS error says nothing useful, so the server checks the length itself.
- Reference implementations worth reading before writing platform code: Clicky (MIT, Swift) for the NSPanel overlay recipe and bezier flight, wayscriber (MIT, Rust) for the entire layer shell approach, wlr-draw for a daemon plus control socket on wlroots.
