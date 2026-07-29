# Building Arin

Nothing is published yet. Until it is, Arin is built from source.

Needs a [Rust toolchain](https://rustup.rs). macOS 14.2 or newer.

```sh
git clone https://github.com/anistark/arin && cd arin
cargo install --path crates/arin-cli
```

That installs one binary, `arin`. It is the daemon, the scripting client, and the MCP
server, chosen by subcommand.

## Running the daemon

```sh
arin daemon
```

It asks for Screen Recording on first run. Arin needs it for two things: noticing when
content moves under a mark, and picking a colour that can be seen against whatever is
underneath. Those frames are compared in memory and are not written anywhere or sent
anywhere. It is the only permission Arin asks for, and in particular it never asks for
Accessibility.

Nothing is drawn until an agent asks.

On a platform with no renderer yet, `arin daemon --headless` runs the whole protocol and
draws nothing, which is how the daemon is exercised before a platform backend exists.

## Distribution

A signed and notarized dmg, a brew tap, GitHub releases and distro packages all land
together at 0.5. Before then, building from source is the only route.
