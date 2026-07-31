# Writing a resolver

A resolver turns `"the Submit button"` into coordinates. Arin ships two: `local`, which
asks a model served on the same machine, and `claude`, which asks a hosted one with your
own API key. This is how to add another.

The whole surface is one trait and one registration call. If adding an adapter needs a
change anywhere else in the tree, that is a bug in the seam rather than in your adapter,
and it is worth reporting.

## Using the ones that ship

```sh
arin resolvers                        # what this build has, and whether each one works
arin daemon --resolver local          # grounds on this machine, sends nothing anywhere
arin daemon --resolver claude         # grounds against a hosted model, uploads screenshots
```

Neither is on by default and neither is chosen by inference. A key in your environment is
not consent, and nor is a model server happening to be running.

`local` speaks the OpenAI shaped `/v1/chat/completions` API, which LM Studio, Ollama, vLLM,
SGLang and llama.cpp's server all serve, so it works with whichever of those you already
have. Load a UI TARS class grounding model and point it at the right port:

| Variable | Default | What it is |
|---|---|---|
| `ARIN_LOCAL_ENDPOINT` | `http://127.0.0.1:1234/v1/chat/completions` | Where your model server listens. Must be loopback. |
| `ARIN_LOCAL_MODEL` | `ui-tars-1.5-7b` | The model name your server knows it by. |
| `ARIN_LOCAL_COORDS` | `pixels` | `normalized` for a model that answers in thousandths, which UI TARS 1.0 does and 1.5 does not. |
| `ARIN_LOCAL_STRUCTURED` | on | Set to `0` for a server that rejects `response_format`. Costs the confidence field, so every mark becomes a region. |

An endpoint that is not `127.0.0.1`, `::1` or `localhost` is refused at startup rather than
used, because `local` reports `is_remote() == false` and that has to be a fact about the
resolver rather than a claim it makes.

A UI TARS checkpoint answers with an action, `click(point='<point>512 384</point>')`,
rather than with a rated JSON object. That is accepted, and it is also why marks from one
come out as regions: an action carries no confidence, so there is no honest number to draw
a precise mark from. A general vision model constrained to the schema reports one and gets
the precise mark.

## The contract

```rust
pub trait Resolver: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn is_remote(&self) -> bool;
    fn detail(&self) -> u32 { arin_core::DEFAULT_DETAIL }
    fn resolve<'a>(&'a self, query: &'a str, frame: &'a Frame)
        -> BoxFuture<'a, Result<Resolution>>;
}
```

You are handed a query and a screenshot. You return a position, optionally a region, and
how sure you are.

```rust
pub struct Resolution {
    pub point: LogicalPoint,
    pub rect: Option<LogicalRect>,
    pub confidence: f64,
}
```

### You do not capture

The daemon captures and hands you the frame. Say how much detail you need through
`detail()` and you will get at least that many pixels on the long edge where the platform
can manage it, or the display's own resolution when it cannot.

This is not a courtesy. An adapter that captured for itself would need a platform
dependency, would break the rule that `arin-resolve` builds anywhere, and could not be
tested without a screen. Yours can be tested with a `Frame` you construct by hand.

### Coordinates are logical points, and the frame is not

`Resolution` is in logical points on the display the frame came from. The frame is in
physical pixels at whatever size the capture produced, which is **not** the display's
resolution and **not** related to `frame.scale` in the way you would expect: a downscaled
capture reports its own pixels per logical point, not the panel's backing scale.

The only honest conversion is between the image your model actually saw and the display it
depicts:

```rust
let logical_x = image_x / image_width as f64 * frame.logical_size[0];
```

If your adapter resizes the frame before showing it to a model, that ratio is against the
size you sent, not the size you were given. Getting this wrong is the single most common
bug in software of this kind, and it fails silently: every mark lands somewhere plausible
and slightly wrong. Do the conversion in exactly one place and test it against a frame
whose dimensions differ from its logical size in both axes.

`arin_resolve::screenshot::Encoded` does this for both shipped adapters and is worth
reading even if you do not use it. It is shared between them on purpose: two adapters
quietly disagreeing about which corner a coordinate is measured from is exactly the bug
this section is about, and one of them is always the one you did not test.

### Confidence decides what gets drawn

The daemon renders a precise mark above `arin_core::policy::HIGH_CONFIDENCE` and outlines
a region below it. That threshold is the reason confidence has to mean something: a
resolver that reports `1.0` for everything has turned the safety net off, and the failure
it protects against is an orb sitting confidently on the wrong button.

If your model does not produce a calibrated number, produce an honest uncalibrated one.
Low is better than wrong. The `local` adapter is the worked example: a UI TARS action has
nowhere to put a confidence, so every one of them gets a constant below the threshold and
draws a region. That is a placeholder for a measurement rather than a measurement, and it
is documented as such where it is defined.

### Not finding it is a legitimate answer

Return an error when the thing is not on screen. Nothing is drawn and the client is told
why. Do not return your best guess with a low confidence and hope the region rendering
covers you, because "somewhere on this screen" is not what a region means.

### `is_remote` is load bearing

Return `true` if any part of resolving sends anything off the machine. This is what the
egress warning and, in time, the consent prompt are built on. A resolver that quietly
returns `false` while making a network call defeats the entire privacy story, and it is
the one thing in this interface that cannot be caught by a test.

## Registering it

```rust
let mut registry = Registry::with_builtins();
registry.register("my-model", || {
    Ok(Arc::new(MyResolver::from_env()?) as Arc<dyn Resolver>)
});
```

The registry holds builders rather than instances, so registering costs nothing and your
adapter is constructed only when someone names it. Do your setup, including reading keys
and opening connections, inside the closure. Failing there is how a user finds out at
startup that something is missing, with your own message, rather than at first use.

## Testing it

Everything except the network is testable without one, and the shipped adapters are laid
out so that it is: `body()` builds a request, `read_answer()` parses a reply, and
`grounding::into_resolution()` converts coordinates, none of which touch a socket.
`resolve()` is a thin thing that calls them in order.

For the part that does need a socket, `crates/arin-resolve/tests/round_trip.rs` stands up
a one-shot HTTP server on loopback and points an adapter at it, which covers the headers,
the body, and the reply for the cost of forty lines and no API key. Copy it. Note that for
`local` this is not a stand-in for anything: loopback HTTP is what it speaks in production,
so the only thing being faked there is the model.

Worth covering, because all of these have been wrong at some point:

- A frame whose pixel dimensions differ from its logical size, in both axes.
- A frame large enough that your adapter resizes it, with the answer converted against the
  size you sent.
- An element the model reports as absent.
- A confidence outside `0.0..=1.0`. Clamp rather than trust.
- A degenerate bounding box. Drop the box and keep the point rather than failing.
- An empty frame, which must fail before anything leaves the machine.

## Checklist

- [ ] `is_remote` is honest.
- [ ] `detail` reflects what the model actually needs.
- [ ] The coordinate conversion is in one place and tested against a non-square ratio.
- [ ] Confidence is meaningful, or honestly low.
- [ ] Not found is an error, not a guess.
- [ ] Construction failures explain themselves.
- [ ] No platform dependency, and the crate still builds on every target.
