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

## Reporting a bug

```sh
arin diagnose
arin diagnose --output ~/arin-report.txt
```

Arin collects no telemetry, so there is nothing on our side to look at when something goes
wrong. `arin diagnose` is the replacement: build and protocol version, the socket and
whether anything is listening on it, the settings a daemon started here would use, every
resolver and whether it can be built, the macOS version, the capture permission, the
displays, and the environment variables Arin reads.

It prints to your terminal on purpose. Nothing is uploaded, and a report you have to open
to see is one people attach without reading. Secrets are never quoted: an API key is
reported as set or not set with its length, which is enough to spot a truncated one without
putting it in a public issue.

One thing it cannot tell you is how a *running* daemon was configured, because nothing on
the wire asks. That section says so, and reports what a daemon started from your shell
right now would use instead.

## Pointing without coordinates

Describe the target instead of measuring it, and the daemon works out where it is.

```sh
arin point "the Submit button"
arin highlight "the error message"
```

This needs a resolver, which is off by default and never turned on by inference. Two ship.
Start the daemon with one by name:

```sh
arin resolvers                       # what this build has, and whether each one works
arin daemon --resolver local         # a model on this machine, nothing leaves it
ANTHROPIC_API_KEY=... arin daemon --resolver claude
```

`arin resolvers` says which are available and, for each, whether it leaves the machine.

`local` talks to a model server you run yourself, over the OpenAI shaped API that LM
Studio, Ollama, vLLM, SGLang and llama.cpp all serve. Load a UI TARS class grounding model,
set `ARIN_LOCAL_ENDPOINT` if it is not on port 1234, and nothing about grounding touches the
network. An endpoint that is not loopback is refused rather than used.

**The `claude` resolver sends data off your machine.** With it on, every `point` or
`highlight` carrying a description uploads a screenshot of that display to Anthropic's API.
Nothing else in Arin sends anything anywhere, and marks made from coordinates never trigger
it. Having an API key in your environment does not switch it on, and the daemon says so at
startup when a resolver that leaves the machine is in use.

### Arin asks before it reads your screen

Grounding is the one thing Arin does that a client could not do for itself. Arin holds
Screen Recording permission and your clients do not, so a client that asks "where is the row
showing the account balance" is reading your screen through Arin's grant. The first time one
does, Arin asks you:

```sh
arin daemon --resolver local                       # asks, and remembers your answer
arin daemon --resolver local --grounding-consent always   # never asks
arin daemon --resolver local --grounding-consent never    # refuses every query
```

The prompt says which client asked, what it asked for, and whether the screenshot leaves the
machine. Allowing for an hour covers anything asked in that time by any program running as
you, and the menu bar shows the grant and takes it back.

**Drawing is never gated.** A program running as you could open its own always-on-top window
and draw on it, so gating that would cost every client a setup step and buy nothing.

With `ask` and no way to prompt, such as `--headless`, the answer is no. A gate that opens
when nobody is watching is not a gate, so an unattended daemon needs
`--grounding-consent always` said out loud.

How the mark is drawn follows how sure the model was. A confident answer puts the orb on
the target. An unsure one outlines the region instead, because a slightly large highlight
reads as intentional and a confident mark on the wrong button reads as broken. A model
that cannot find the thing at all says so, and nothing is drawn.

Accuracy is not measured yet. There is no eval set behind grounding, so treat
`arin point "the Submit button"` as something to try rather than something to rely on, and
treat any comparison between the two resolvers as a guess until there is one.

See [resolvers.md](resolvers.md) for writing an adapter.

## Choosing the colour marks come out in

Marks are amber by default, and the daemon moves off it when amber cannot be seen against
what is under the mark. Both halves of that are configurable on the daemon:

```sh
arin daemon --color '#FF2D95'                      # draw marks in magenta
arin daemon --palette '#FF2D95,#30D158,#F5F5F7'    # replace the whole fallback set
arin daemon --no-adaptive-color                    # never look at the screen, never move
```

`--color` changes what marks are drawn in and keeps the built-in fallbacks, which is what
naming one colour almost always means. `--palette` replaces the set outright, first entry
preferred, and takes precedence over `--color`. Both read from `ARIN_COLOR` and
`ARIN_PALETTE`. A single message can still override everything with `--color` on
`arin draw`.

`--no-adaptive-color` saves a screen capture per positioned annotation, at the cost of the
record of what each mark was drawn over, which is what lets a mark following a scroll be
checked against where it landed.

**Blue is refused.** It belongs to the orb, and a mark in the orb's own colour reads as
part of the orb rather than as a separate thing. A palette containing one is rejected at
startup with the hue range and an explanation, rather than accepted and quietly stripped:
silently dropping the colour you asked for leaves you watching marks come out amber with
nothing to explain it.
