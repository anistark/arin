# arin

The Rust API for [Arin](https://github.com/anistark/arin), an annotation layer any agent
can draw on.

## Installing Arin itself

**This crate is not the application.** Arin is a daemon that draws on your screen, and it
is installed as an application:

```sh
brew install --cask arin
```

`cargo install arin` will not work. There is no binary in this crate, by design. See the
[project README](https://github.com/anistark/arin) for the other install paths, including
building from source.

## What this crate is

The contract for talking to the daemon from Rust. Arin points at things, highlights
regions, writes explanatory text boxes, and draws freehand paths. It never clicks, types,
or scrolls. This is the type definitions for the messages that ask it to.

```toml
[dependencies]
arin = "0.1"
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

## Two names for it

Everything here is [`arin-protocol`](https://crates.io/crates/arin-protocol) re-exported.
Depend on whichever name reads better where you are using it: `arin` if you are writing a
client and think of it as talking to Arin, `arin-protocol` if you are implementing the
wire format somewhere else and think of it as a specification. They do not drift, because
this crate tracks the protocol within a compatible range.
