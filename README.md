<div align="center">

<img src="assets/logo.png" width="150" alt="Arin">

# ARIN

**An annotation layer any agent can draw on.**

Arin is a local daemon that draws on your screen. Point at things, highlight regions,
write explanations. Any AI can drive it over MCP or a small JSON protocol.

It draws. It never clicks.

</div>

---

## What it does

An agent explaining an article can circle each section as it talks about it. An agent
reviewing your code can point at the line it means. A tutorial agent can show you where
a button is instead of describing it.

Arin holds no model and makes no decisions. The intelligence is your agent. Arin is the
chalk.

## Why draw only

Arin never synthesises input. No clicks, no keystrokes, no scrolling. That boundary is
the point: a tool that can only render pixels is safe to leave running, needs no
Accessibility permission, and is auditable in an afternoon. Agents that need to act
should compose Arin with a separate actuator.

## Install

Needs a [Rust toolchain](https://rustup.rs). macOS 14.2 or newer.

```
git clone https://github.com/anistark/arin && cd arin
cargo install --path crates/arin-cli
```

Then start the daemon. It asks for Screen Recording on first run, which it needs to
notice when the page scrolls under a mark. Nothing is drawn until an agent asks.

```
arin daemon
```

## Use from an agent

Add it to your MCP client. For Claude Code that is one line:

```
claude mcp add arin -- arin mcp
```

Or, in any client that takes the standard JSON:

```json
{
  "mcpServers": {
    "arin": { "command": "arin", "args": ["mcp"] }
  }
}
```

That exposes four tools. They are named after what an agent is trying to do rather than
after the message underneath, so a model reaches for the right one without being told.

| Tool | What it does |
|---|---|
| `point_at` | Puts the orb on a position, with an optional caption |
| `highlight` | Outlines a region, with an optional caption |
| `annotate` | Places a block of explanatory text |
| `clear` | Removes one mark, or every mark the agent drew |

Every call reports back the display's size and scale, so an agent working from a
screenshot can convert pixels to logical points without asking twice. Marks live until
they are cleared, the content scrolls, or the client disconnects. Pass `ttl_seconds` to
have one remove itself instead.

## Drive it from a shell

The CLI speaks the same protocol an agent would, which makes it the quickest way to see
what Arin does.

```
arin displays
arin point 412 88 --display 1 --label Save --hold
arin point --at top-right --label "the close button"
arin highlight 100 200 340 90 --label "the counterargument" --ttl 5
arin annotate 300 200 320 80 --text "This is where the retry loop lives"
arin draw 100,200 140,210 180,190 --color '#FF3B30'
```

`--hold` keeps a mark up until you interrupt it, since annotations live only as long as
the session that made them and a one-shot command ends its session on the way out.
`--ttl` takes seconds. On a platform with no renderer yet, `arin daemon --headless` runs
the whole protocol and draws nothing.

## Pointing without coordinates

Describe the target instead of measuring it, and the daemon works out where it is.

```
arin point "the Submit button"
arin highlight "the error message"
```

This needs a resolver, which is off by default and never turned on by inference. Start the
daemon with one by name:

```
arin resolvers
ANTHROPIC_API_KEY=... arin daemon --resolver claude
```

`arin resolvers` says which are available and, for each, whether it leaves the machine.
The one that ships takes a screenshot of the display and sends it to Anthropic's API on
every query, so it is worth being deliberate about: having a key in your environment does
not switch it on, and the daemon warns on startup when a resolver that sends anything
anywhere is in use.

How the mark is drawn follows how sure the model was. A confident answer puts the orb on
the target. An unsure one outlines the region instead, because a slightly large highlight
reads as intentional and a confident mark on the wrong button reads as broken. A model
that cannot find the thing at all says so, and nothing is drawn.

## The protocol

Newline-delimited JSON over a Unix domain socket. The socket is mode 0600 inside a
directory only its owner can traverse, every connection has its peer credentials checked
before a byte is read, and there is no network listener.

```json
{"v":"0.1","type":"session_start","client_name":"claude-code"}
{"v":"0.1","type":"point","x":412,"y":88,"display_id":1,"label":"Save"}
{"v":"0.1","type":"point","at":"top-right","display_id":1,"label":"close"}
{"v":"0.1","type":"highlight","rect":[100,200,340,90],"display_id":1,"label":"the counterargument"}
{"v":"0.1","type":"textbox","rect":[300,200,320,80],"display_id":1,"text":"the retry loop"}
{"v":"0.1","type":"draw","display_id":1,"path":[[100,200],[140,210]],"ttl_ms":5000}
{"v":"0.1","type":"clear","all":true}
{"v":"0.1","type":"session_end"}
```

The daemon replies to each with an `ack` carrying the annotation's id and the display it
landed on, or an `error` naming a machine-readable code.

Separately, it pushes an `invalidated` whenever one of your marks goes away for a reason
you did not ask for: `scroll`, `ttl`, `cleared`, or `display_change`. These arrive when
they happen rather than in answer to anything, so read until you see your own reply and
set aside any `invalidated` you pass. You are only ever told about your own marks. Over
MCP this arrives as a `gone` field on the next tool result, since there is no way for a
server to interrupt a model mid-thought.

Every coordinate is a logical point paired with an explicit display. Never physical
pixels: a Retina screenshot is 2x the logical size, and mixed-DPI multi-monitor setups
make implicit conversion unrecoverable. Clients working from a screenshot divide by the
scale reported in the ack.

A client that has not measured the screen can name a position instead of a coordinate:
one of `top-left`, `top`, `top-right`, `left`, `center`, `right`, `bottom-left`, `bottom`,
`bottom-right`, or a pair like `50%,30%`. The daemon resolves it against the display,
since the daemon is the one that knows how big the display is. Names are approximate by
design, so anything needing precision sends coordinates.

Clients that can ground coordinates themselves send them directly. Clients that cannot
will be able to send a natural language query instead, resolved by a pluggable grounding
model. That lands in 0.3.

## Status

Pre-release, and honest about it. On macOS everything below works: the overlay is click
through and never takes focus, all four annotation kinds draw, the orb flies to its
target and trails embers, marks follow content that scrolls and are dropped when they
cannot, and marks can be cleared from the menu bar or with `Cmd+Shift+K`.

Grounding is the newest part and the least proven. Every failure path is covered by tests
and its accuracy is not measured at all: there is no eval set behind it yet, so treat
`arin point "the Submit button"` as something to try rather than something to rely on.

| Platform | Status |
|---|---|
| macOS 14.2+ | works |
| Linux, KDE and wlroots | 0.4 |
| Windows | 0.6 |
| Linux, GNOME | not supported, no layer shell |

Nothing is tagged before 1.0. The 0.x line ships from `main`.

## Privacy

Arin has no analytics, no accounts, and no network listener. The daemon binds a Unix
domain socket and nothing else.

There is exactly one thing that sends anything off the machine: a grounding resolver, and
only while one is configured. `arin daemon` on its own reaches nothing, an API key in your
environment does not enable anything, and `arin resolvers` tells you which of them leave
the machine before you pick one. With the Claude resolver on, every `point` or `highlight`
carrying a description uploads a screenshot of that display to Anthropic's API. Nothing
else does, and marks made from coordinates never trigger it. Local grounding lands in 0.5
and removes the requirement entirely.

Screen Recording is the only permission Arin asks for. It is used to notice when content
moves under a mark and to choose a colour that can be seen against what is under it. Those
frames are compared in memory and never leave the machine.

## License

MIT
