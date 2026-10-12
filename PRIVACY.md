# Privacy

Arin runs on your Mac, and what it sees stays there. It has no accounts, no analytics and no crash reporting. With its default settings it makes no network connections at all.

Two settings change that, and both are off until you turn them on: the `claude` resolver and the update check. Both are covered under [What leaves your machine](#what-leaves-your-machine).

## What Arin reads

**Your screen.** Arin holds the macOS Screen Recording permission and captures a display to:

- check whether content scrolled under a mark. This runs every 500ms, and only while an agent is connected.
- pick a colour that stands out against what a mark is drawn over. `--no-adaptive-color` turns this off.
- find the thing a client described, such as `"the Submit button"`. This only happens when you configured a resolver and allowed grounding.

Captures are held in memory and thrown away. Each mark keeps a coarse fingerprint of what it was drawn over, so a mark that follows a scroll can be checked against where it landed. That is gone with the mark.

**Your window list.** When an agent asks Arin to bring an application forward or to wait for one, Arin matches the request against application names and window titles. The client is told which application was raised and nothing else. It never learns a title, or what else you have open.

**Chrome's open tabs, only with `--read-browser-tabs`.** Arin reads the titles and addresses of open tabs from Chrome's session files on disk, for profiles used within the last hour, so it can find a tab you are not looking at. They are matched inside the daemon and are never sent to a client, written to the log, or sent off your machine.

**What clients send it.** The marks to draw, their text, and the name the client gives itself.

## What a client learns

A client is a program that talks to Arin, such as your coding agent. It gets back the ids of the marks it drew, the size and scale of the display it drew on, and why a mark was removed. If you configured a resolver, a described target also comes back with where it was found and how sure the model was.

That last one is screen content, so Arin asks you before it grounds anything: `--grounding-consent` is `ask` by default, and a daemon with no way to ask you refuses. Your answer lives in memory and does not survive a restart. Drawing is never gated, because any program running as you could open a window and draw on it anyway.

Only programs running as your own user can connect. The socket is a Unix domain socket with mode `0600`, and the daemon checks every peer's user id before reading from it. Arin never listens on a network port.

## What leaves your machine

Nothing, unless you turn on one of these:

| Setting | When it sends | Where to | What |
|---|---|---|---|
| `--resolver claude` | Each `point` or `highlight` that describes its target instead of giving coordinates | Anthropic's API, using your own API key | A screenshot of that display and the description |
| `--check-updates` | Once a day | GitHub's API | A request for the latest release tag, with `arin/<version>` as its user agent |

`arin update` makes the same request as `--check-updates`, once, when you run it.

The daemon logs a warning at startup when a resolver that sends data off the machine is on, and again each time a screenshot goes out. An API key in your environment does not turn the `claude` resolver on. It has to be named.

What Anthropic does with a screenshot is covered by your agreement with them, not by this file. GitHub sees your IP address when the update check runs, as it would for any request.

The `local` resolver talks to a model server on your own machine. It refuses any address that is not `127.0.0.1`, `::1` or `localhost`, so it cannot be pointed somewhere else by mistake.

## What Arin stores

| Where | What | When |
|---|---|---|
| `~/Library/Logs/Arin/arin.log` | Startup settings, client names, grounding decisions, which application was brought forward, and errors | When Arin is started by the login agent |
| `~/Library/Application Support/Arin` | Whether this build has asked macOS for Screen Recording, and which build capture last worked for: its version and the path to its binary | Always |
| `~/Library/LaunchAgents/com.anistark.arin.plist` | The login agent | After `arin service enable` |

The log does not record screen contents, window titles, tabs, descriptions sent for grounding, or the text of marks.

Marks, your grounding answer and captures are kept only in memory. Strokes you draw with the marker never leave the overlay: the daemon does not know they exist, so neither does any client.

`ARIN_RECORD` is a development aid that writes raw screen captures to a directory you name, to calibrate scroll detection against. It is off unless that variable is set, logs a warning while it runs, and stops after 200 pairs.

## Your agent is separate

Arin holds no model. The agent driving it has its own privacy terms, and what it does with your conversation, or with screenshots it takes itself, is between you and whoever provides it. Arin does not hand an agent screenshots: no tool reads the screen.

## Getting Arin

Homebrew, GitHub and crates.io see a download the way they see any other, under their own policies. Homebrew collects its own analytics, which `brew analytics off` turns off. The docs site is hosted on GitHub Pages, runs no analytics, and loads its fonts from Google Fonts.

## Reporting a bug

`arin diagnose` prints a report to your terminal and uploads nothing. API keys are reported as set or not set, with their length, and never quoted. The report does include your macOS version, your display arrangement and some file paths, so read it before attaching it to a public issue.

## Removing what Arin left

```sh
arin service disable
rm -rf ~/Library/Logs/Arin "$HOME/Library/Application Support/Arin"
tccutil reset ScreenCapture com.anistark.arin
```

[Uninstalling](docs/install.md#uninstalling) covers removing the app itself.

## Changes

Changes to this file are recorded in [CHANGELOG.md](CHANGELOG.md) and in its git history. Questions go to [GitHub issues](https://github.com/anistark/arin/issues).
