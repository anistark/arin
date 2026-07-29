# Driving Arin from a shell

The CLI speaks the same protocol an agent would, which makes it the quickest way to see
what Arin does and the quickest way to check that a change works.

```sh
arin displays
arin point 412 88 --display 1 --label Save --hold
arin point --at top-right --label "the close button"
arin highlight 100 200 340 90 --label "the counterargument" --ttl 5
arin annotate 300 200 320 80 --text "This is where the retry loop lives"
arin draw 100,200 140,210 180,190 --color '#FF3B30'
arin clear
```

`--hold` keeps a mark up until you interrupt it. Annotations live only as long as the
session that made them, and a one-shot command ends its session on the way out, so
without `--hold` the mark goes as soon as the command returns.

`--ttl` takes seconds and has the mark remove itself.

`arin status` reports whether the daemon is reachable. `arin displays` lists the displays
with the ids to pass to `--display`.

## Pointing without coordinates

Describe the target instead of measuring it, and the daemon works out where it is.

```sh
arin point "the Submit button"
arin highlight "the error message"
```

This needs a resolver, which is off by default and never turned on by inference. Start the
daemon with one by name:

```sh
arin resolvers
ANTHROPIC_API_KEY=... arin daemon --resolver claude
```

`arin resolvers` says which are available and, for each, whether it leaves the machine.

**The resolver that ships sends data off your machine.** With the Claude resolver on,
every `point` or `highlight` carrying a description uploads a screenshot of that display
to Anthropic's API. Nothing else in Arin sends anything anywhere, and marks made from
coordinates never trigger it. Having an API key in your environment does not switch it on,
and the daemon says so at startup when a resolver that leaves the machine is in use.

How the mark is drawn follows how sure the model was. A confident answer puts the orb on
the target. An unsure one outlines the region instead, because a slightly large highlight
reads as intentional and a confident mark on the wrong button reads as broken. A model
that cannot find the thing at all says so, and nothing is drawn.

Accuracy is not measured yet. There is no eval set behind grounding, so treat
`arin point "the Submit button"` as something to try rather than something to rely on.

See [resolvers.md](resolvers.md) for writing an adapter.
