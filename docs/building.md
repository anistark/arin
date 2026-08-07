# Building Arin

For working on Arin rather than with it. If you only want to run it, [Install](/docs/install/)
is shorter.

Needs a [Rust toolchain](https://rustup.rs) and macOS 14 or newer. The platform code is
macOS only, and the core is not: `arin-core` and `arin-protocol` build and test on Linux
with no platform crate in the tree, which is what keeps the v2 ports cheap.

```sh
git clone https://github.com/anistark/arin && cd arin
cargo build --workspace
cargo test --workspace
```

The whole test suite is headless. Platform behaviour arrives through traits and the tests
wire up fakes, so nothing wants a display and nothing wants a permission.

## The task runner

[`just`](https://just.systems) has the recipes, and `just` on its own lists them.

```sh
just dev        # the daemon with no renderer
just run point 412 88 --display 1 --label Save
just test
just lint       # fmt and clippy, at the strictness CI uses
just ci         # everything CI runs, in the order it runs it
```

`just ci` passing locally means CI passes, which is the point of it existing.

A Nix shell with the toolchain and these tools is `nix develop`. It is offered rather than
required: Arin is developed with rustup and the system Xcode, and what the shell has to
stay is one in which `just ci` passes.

## Running it while you work

```sh
cargo run --bin arin -- --socket /tmp/a.sock daemon --headless
cargo run --bin arin -- --socket /tmp/a.sock point 412 88 --display 1 --label Save
```

`--headless` runs the socket, the protocol, and the whole state machine, and draws nothing.
It is how the daemon is exercised without a display, and how a platform backend that does
not exist yet is worked around.

A custom `--socket` keeps a development daemon out of the way of an installed one. Unix
socket paths run out at around 104 bytes, so keep it short.

For the real renderer you want the bundle, because the menu bar item and the Screen
Recording grant are properties of it:

```sh
just bundle
open target/bundle/Arin.app
```

Two bundles carrying one identifier compete for a single Screen Recording record, so if a
development build and an installed one are both on the machine, expect to
`tccutil reset ScreenCapture com.anistark.arin` between them. `just bundle` warns when it
notices the other one.

## The invariants

Three things CI enforces, each protecting a promise rather than a preference.

```sh
just core        # core and the protocol stand alone, with no platform crate
just draw-only   # no input synthesis API is referenced anywhere
just lint        # fmt, and clippy with warnings denied
```

**`draw-only` is the product boundary.** Arin never synthesises input: no clicks, no
keystrokes, no scrolling. That is what keeps the permission surface to Screen Recording
alone and what makes the thing safe to leave running. If that check fires on something you
wrote, the feature belongs in a different project.

**`core` is what keeps the ports cheap.** A macOS dependency in `arin-core` breaks it, and
that is the intended outcome rather than an inconvenience.

## Where things live

| Crate | What it is |
|---|---|
| `arin-protocol` | Message types, validation, version negotiation. No IO. |
| `arin-core` | The daemon. Socket, sessions, annotation state, anchors, scroll handling. |
| `arin-resolve` | The resolver registry and its adapters. The only crate that reaches the network. |
| `arin-mac` | The renderer and capture, in objc2. NSPanel, Core Animation, ScreenCaptureKit. |
| `arin-mcp` | The MCP server, served by `arin mcp`. |
| `arin-cli` | The `arin` binary, which is the only one. |
| `arin-linux`, `arin-win` | Empty scaffolds for v2. |

One binary, on purpose. MCP is `arin mcp` rather than a second executable, because an
agent's config is written once and outlives several updates.

**Every coordinate in the protocol is a logical point with an explicit display id, never a
physical pixel.** Conversion happens inside a platform crate and nowhere else. If you find
yourself dividing by a scale factor outside one, something is wrong.

## Before you send a patch

[`AGENTS.md`](https://github.com/anistark/arin/blob/main/AGENTS.md) is the context file:
the architecture, the decisions already made and why, and the conventions. It is worth
reading before a first change, and worth updating in the same commit as a change that
contradicts it.

Branches are `feat/`, `fix/`, `docs/`, `refactor/` or `chore/` over a topic. Notable
changes go under `[Unreleased]` in the changelog in the same branch as the change.

Two writing rules that apply to code comments as much as prose: no em-dashes and no
semicolons in prose, and comments explain why rather than narrate what the next line does.
