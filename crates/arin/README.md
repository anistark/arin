# arin

The Rust API for [Arin](https://github.com/anistark/arin), an annotation layer any agent
can draw on.

## Installing Arin itself

**This crate is not the application, and `cargo install arin` will not work.** There is no
binary here, by design. Arin is a menu bar app and a daemon that draws on your screen:

```sh
brew install anistark/tools/arin
```

That compiles from source, so it wants a Rust toolchain and a couple of minutes. Building
locally is deliberate rather than lazy: an unsigned app that was *downloaded* is
quarantined and refused by Gatekeeper, while one compiled on your own machine is not. A
signed build shipping as a cask comes later, and the install line becomes
`brew install --cask anistark/tools/arin` when it does.

Or build it directly:

```sh
git clone https://github.com/anistark/arin
cd arin
just bundle          # produces target/bundle/Arin.app
```

Either way, start it and grant Screen Recording when asked:

```sh
arin -d              # the daemon, in the foreground
```

macOS 14 or later. The daemon is macOS-only today; this crate builds anywhere.

## What this crate is

The contract for talking to the daemon from Rust. Arin points at things, highlights
regions, writes explanatory text boxes, and draws freehand paths. It never clicks, types,
or scrolls. This is the type definitions for the messages that ask it to.

```toml
[dependencies]
arin = "0.2"
```

It is pure types plus validation. No IO, no async runtime, no platform code, and it builds
anywhere.

```rust
use arin::{Anchor, DisplayId, LogicalRect};

let rect = LogicalRect::new(412.0, 88.0, 120.0, 32.0);
assert!(rect.is_valid());

let anchor = Anchor::new(rect, DisplayId(1));
assert_eq!(anchor.display_id, DisplayId(1));
```

**The crate version is not the wire version.** This crate is 0.2 while the protocol it
describes is 0.1, and they move independently: a Rust API change bumps the crate, a wire
format change bumps `PROTOCOL_VERSION`. Check compatibility against that constant rather
than against the version in your `Cargo.toml`.

## Two names for it

Everything here is [`arin-protocol`](https://crates.io/crates/arin-protocol) re-exported.
Depend on whichever name reads better where you are using it: `arin` if you are writing a
client and think of it as talking to Arin, `arin-protocol` if you are implementing the
wire format somewhere else and think of it as a specification. They do not drift, because
this crate tracks the protocol within a compatible range.

## Licence

MIT.
