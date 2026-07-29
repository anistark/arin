# The protocol

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

## Invalidations

Separately, the daemon pushes an `invalidated` whenever one of your marks goes away for a
reason you did not ask for: `scroll`, `ttl`, `cleared`, or `display_change`.

These arrive when they happen rather than in answer to anything, so read until you see
your own reply and set aside any `invalidated` you pass on the way. You are only ever told
about your own marks.

## Coordinates

Every coordinate is a logical point paired with an explicit display. Never physical
pixels: a Retina screenshot is 2x the logical size, and mixed-DPI multi-monitor setups
make implicit conversion unrecoverable. Clients working from a screenshot divide by the
scale reported in the ack.

A client that has not measured the screen can name a position instead of a coordinate:
one of `top-left`, `top`, `top-right`, `left`, `center`, `right`, `bottom-left`, `bottom`,
`bottom-right`, or a pair like `50%,30%`. The daemon resolves it against the display,
since the daemon is the one that knows how big the display is. Names are approximate by
design, so anything needing precision sends coordinates.

Clients that cannot ground coordinates themselves can send a natural language query
instead, resolved by a pluggable grounding model. See [cli.md](cli.md).

## In Rust

The wire types are published as [`arin`](https://crates.io/crates/arin), or under
[`arin-protocol`](https://crates.io/crates/arin-protocol) if you would rather name the
specification than the tool. They are the same crate.

```toml
[dependencies]
arin = "0.1"
```
