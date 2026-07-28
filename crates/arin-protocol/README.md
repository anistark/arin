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
arin-protocol = "0.1"
```

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

The protocol is not frozen. It freezes at Arin 1.0, after which changes are additive only.
Until then the crate follows SemVer as a Rust API, but the wire format it describes may
still move. The open question blocking the freeze is authentication, which affects the
`session_start` handshake.

## Licence

MIT.
