# arin-protocol

Wire protocol types for [Arin](https://github.com/anistark/arin), an annotation layer any
agent can draw on.

Arin is a local daemon that draws on the user's screen: it points at things, highlights
regions, writes explanatory text boxes, and draws freehand paths. It never clicks, types,
or scrolls. This crate is the contract between the daemon and whatever is driving it.

It is pure types plus validation. No IO, no async runtime, no platform code, and it builds
anywhere. Use it to speak to the daemon from Rust without hand-rolling the JSON, or to
implement the protocol somewhere else with the type definitions as the reference.

```toml
[dependencies]
arin-protocol = "0.2"
```

**The crate version is not the wire version.** This crate is 0.2 while the format it
describes is 0.1, and they move independently: a Rust API change bumps the crate, a wire
format change bumps `PROTOCOL_VERSION`. Compatibility checks go against that constant, not
against the version in your `Cargo.toml`.

## The shape of it

JSON lines over a Unix domain socket, one object per line, UTF-8. Every message carries a
`v` field.

```rust
use arin_protocol::{ClientMessage, Envelope, PROTOCOL_VERSION};

let line = r#"{"v":"0.1","type":"point","x":412.0,"y":88.0,"display_id":1}"#;
let message: Envelope<ClientMessage> = serde_json::from_str(line)?;
assert!(message.version.is_compatible_with(PROTOCOL_VERSION));
# Ok::<(), serde_json::Error>(())
```

Client to daemon: `session_start`, `point`, `highlight`, `textbox`, `draw`, `clear`,
`session_end`. Daemon to client: `ack`, `invalidated`, `error`.

## Two rules worth knowing before you use it

**Every coordinate is a logical point paired with an explicit `display_id`.** Never
physical pixels. A Retina screenshot is twice the logical size, and mixed-DPI multi-monitor
setups make implicit conversion unrecoverable. Clients working from a screenshot divide by
the scale factor reported in the `ack` before sending. There is no implicit primary
display.

**Unknown fields are ignored and unknown message types produce an `error` rather than a
closed connection**, so a client built against a later minor version keeps working.
Changes are additive within a major version, and reserved fields exist so that stays true.

## Stability

The protocol is not frozen. Until it is, the crate follows SemVer as a Rust API while the
wire format it describes may still move.

**Authentication no longer blocks the freeze.** That was the open question, and it is
settled: permission is a capability split rather than a handshake. Drawing is open to any
process running as you, since one could draw on its own window anyway. Grounding, which is
the only thing Arin can do that its clients cannot, is gated behind a consent prompt. None
of that changed `session_start`, so nothing here moved.

What is still open is *when* to freeze. Freezing is a promise to people writing clients,
and a second implementation is what proves a protocol is a protocol rather than a
description of one renderer. Only one renderer has ever spoken this format, so the freeze
may wait for a second one rather than landing with Arin 1.0. Assume additive-only changes
are not guaranteed until this section says otherwise.

## Licence

MIT.
