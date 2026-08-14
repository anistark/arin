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

## Keeping the daemon running

`arin -d` runs in the foreground and stops on Ctrl-C. To have Arin there after a reboot,
install the launch agent:

```sh
arin service enable
```

It works out which `Arin.app` to run from the binary you typed it with, so the line is the
same however Arin was installed. `arin service status` says whether the agent is installed
and which build it starts, and exits non-zero when there is none, so a setup script can
ask. `arin service restart` is what to run after `brew upgrade`: the agent survives an
upgrade, a daemon that is already running does not get replaced by one. `arin service
disable` stops it starting at login and leaves the app alone.

[Install](/docs/install/) has the rest, including what happens with Nix, which manages the
same agent through `services.arin.enable` instead.

## Checking the screen recording permission

```sh
arin permissions
arin permissions --open
```

Arin needs Screen Recording to notice when content moves under a mark and to pick a colour
that reads against whatever is underneath. Marks still draw without it. They just stop
following the page.

`arin permissions` proves the permission by taking a frame rather than trusting what macOS
reports, because those two disagree in a way that matters: the system reports the grant the
moment you flip the switch, and ScreenCaptureKit serves nothing to a process that was
already running when that happened. When they disagree the answer is to restart Arin, and
nothing else will tell you so. It exits non-zero when capture does not work, so a setup
script can gate on it. `--open` goes straight to the switch.

When something is wrong it also reports whether this build has an identity a grant can
attach to at all. macOS remembers a grant against a code signature rather than against a
name, so a build whose signature does not verify reports exactly what a build nobody has
granted reports, and only one of those is fixed in System Settings. If that line says the
signature does not verify, it prints the two commands that clear it. See
[Install](/docs/install/#when-permissions-go-wrong).

**Run beside a live daemon, this command changes what it answers.** Only one process can
hold the capture stream, and macOS attributes a permission to whatever launched the process
asking, which from a terminal is the terminal. So a granted answer here would be your
terminal's grant wearing Arin's name. When the daemon is up, `arin permissions` says so and
points at the daemon's log, which is the only honest source:

```sh
tail ~/Library/Logs/Arin/arin.log
```

The daemon says on startup whether capture works. If that file does not exist and the launch
agent is enabled, launchd has been discarding the log: `arin service status` reports it, and
`arin service enable` creates the directory.

## Reporting a bug

```sh
arin diagnose
arin diagnose --output ~/arin-report.txt
```

Arin collects no telemetry, so there is nothing on our side to look at when something goes
wrong. `arin diagnose` is the replacement: build and protocol version, the socket and
whether anything is listening on it, the settings a daemon started here would use, every
resolver and whether it can be built, the macOS version, the capture permission and the
signing identity behind it, the displays, and the environment variables Arin reads.

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

## Reaching a window on another desktop

Arin can only mark the desktop in front of you. Every desktop on a display shares one set of
coordinates, so a mark aimed at a window you have swiped away from lands on the desktop
actually in front of you, over unrelated content, and is taken down as you move. From your
side that looks like Arin never firing.

The fix is to bring the application forward first, which needs a daemon started for it:

```sh
arin daemon --allow-activation
arin focus slack                     # by name, loosely matched
arin focus com.tinyspeck.slackmacgap # or by bundle identifier
arin focus gmail                     # or by what a window is showing
```

Then point at what is in it. Agents get the same thing as a `bring_to_front` tool.

Only applications holding a window can be raised, and a name matching several is refused
rather than guessed at.

**The name does not have to be an application.** Nobody has an app called Gmail, so Arin
also matches what your windows are showing: `arin focus gmail` finds the browser window
whose tab says Gmail and raises that browser. Naming an application always wins over a
window that merely mentions it, so `arin focus slack` reaches Slack even when a browser tab
has "Slack" in its title. Only the tab you are looking at in each window counts, since a
background tab is not in any window's title.

A refusal never says which applications it matched, how many, or what any window is showing.
Arin can see your windows and the program asking cannot, so an error describing them would
be a way to read your screen through Arin's permission, one call at a time. Window titles
are matched against inside the daemon and never reported to anyone.

### Finding a tab you are not looking at

Window titles only reach the tab you have in front in each window, so an inbox sitting two
tabs deep in another Chrome profile is invisible. Add `--read-browser-tabs` and Arin will
also match against Chrome's open tabs:

```sh
arin daemon --allow-activation --read-browser-tabs
arin focus mail.google.com   # Google Chrome — in the "Your Chrome" profile window
```

It reads Chrome's own session files from disk. No permission is involved and nothing is
sent anywhere, but it is the most private thing Arin looks at, so it is off unless you ask
and it is separate from `--allow-activation`. Only profiles used in the last hour are read,
so a profile you closed this morning is not treated as open.

Because raising a browser cannot switch profile, the answer names the window to look in.
That is as far as it goes: Arin will not switch profiles or select the tab for you.

Matching is literal, not clever. If your mail says `Inbox - you@company.com - Company Mail`
at `mail.google.com`, then `arin focus gmail` finds nothing and `arin focus mail` finds it.
Working out that "email" means `mail.google.com` is the job of whatever is driving Arin.

**This is off by default, and not because it is dangerous to your data.** Anything running
as you can already bring an application forward, with no permission of any kind, and Arin
uses the same call the Dock does. It is off because it is disruptive: it changes what you
are looking at, it follows you across desktops, and whatever you were typing goes to
whatever came forward. Arin's whole claim is that it can be left running because it only
ever draws, so a client that can rearrange what is in front of you is something you should
have to ask for.

**Arin activates and nothing else.** No window is moved, resized, closed, or arranged. All
of those need the Accessibility permission, and Arin never asks for it. Screen Recording
remains the only permission it wants.

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
