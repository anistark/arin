# Quickstart

Arin is a daemon that draws on your screen. Your agent tells it where to point, and it
points. It draws and it never clicks, so there is nothing it can do to your machine that
you cannot see happening.

Four steps, about five minutes, most of it a compile.

## 1. Install it

```sh
brew install anistark/tools/arin
```

That builds from source, which takes a minute or two. [Install](/docs/install/) covers why,
and covers Nix, the dmg, and building it yourself.

## 2. Start the daemon

```sh
arin -d
```

macOS asks for Screen Recording the first time. Arin needs it to notice when content moves
under a mark and to pick a colour that can be seen against whatever is underneath. It is
the only permission Arin ever asks for, and it never asks for Accessibility.

Nothing is drawn until something asks. Leave it running.

## 3. Point at something

In another terminal:

```sh
arin displays
arin point 412 88 --display 1 --label Save --hold
```

The orb flies to that spot and stays until you interrupt it. If you see it, everything
below works.

```sh
arin clear
```

## 4. Give it to an agent

```sh
claude mcp add arin -- arin mcp
```

Now ask your agent to explain something on screen. It has four tools: point at a position,
highlight a region, annotate with a block of text, and clear. [MCP](/docs/mcp/) has the
details, and [CLI](/docs/cli/) is the faster way to explore what the marks look like.

## What next

- **[Install](/docs/install/)** for the other ways in, starting at login, and uninstalling.
- **[CLI](/docs/cli/)** for every mark Arin can draw.
- **[Resolvers](/docs/resolvers/)** if you want to say "the Submit button" instead of a
  coordinate. Off unless you turn it on, because it means letting Arin read your screen.
- **[Building](/docs/building/)** if you would rather work on it than use it.
