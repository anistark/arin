# Using Arin from an agent

Arin speaks MCP over stdio. An MCP client launches `arin mcp` as a subprocess, so any
client that can run a local stdio server can drive it. Most of them take the same JSON:

```json
{
  "mcpServers": {
    "arin": { "command": "arin", "args": ["mcp"] }
  }
}
```

`arin mcp` starts whether or not the daemon is running, and connects to its socket on the
first tool call. A call made with no daemon comes back as an error naming the socket, and
the next call after the daemon starts goes through, with no need to restart the agent. The
same goes for a daemon that restarts partway through a session.

## Setting up a client

Whichever client you use, register Arin for your user rather than for one project. Arin
draws on your screen rather than on a repository, so a server registered against one
directory leaves every session started anywhere else with no Arin at all: no tools, and no
way to know it was meant to have any.

A client launched as an app rather than from a terminal may not see your shell's `PATH`,
and then it cannot find `arin`. If one says it cannot start the server, replace `arin` with
the full path that `command -v arin` prints. A Homebrew install puts it at
`/opt/homebrew/bin/arin` on Apple silicon and `/usr/local/bin/arin` on Intel. The dmg puts
it nowhere until you [link it yourself](/docs/install/#the-dmg).

### Claude Code

```sh
claude mcp add --scope user arin -- arin mcp
```

`--scope user` is the part worth getting right. Leave it off and the server is registered
against whichever directory you happened to run the command in.

### Claude Desktop

Add Arin to `~/Library/Application Support/Claude/claude_desktop_config.json`, which
Settings, then Developer, then Edit Config opens for you. Claude Desktop starts from the
Dock without your shell's `PATH`, so give the full path:

```json
{
  "mcpServers": {
    "arin": { "command": "/opt/homebrew/bin/arin", "args": ["mcp"] }
  }
}
```

Quit and reopen the app to load it.

### Cursor

The standard JSON above, in `~/.cursor/mcp.json`. A `.cursor/mcp.json` inside a project
works too, for that project alone.

### Codex

```sh
codex mcp add arin -- arin mcp
```

That writes `~/.codex/config.toml`, which the Codex CLI, the IDE extension and the desktop
app all read. By hand it is:

```toml
[mcp_servers.arin]
command = "arin"
args = ["mcp"]
```

`/mcp` inside a session lists the servers Codex connected to.

### opencode

opencode keeps servers under `mcp` rather than `mcpServers`, and takes the command as one
array. In `~/.config/opencode/opencode.json`:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "arin": { "type": "local", "command": ["arin", "mcp"], "enabled": true }
  }
}
```

The OpenCode 2 docs show servers nested one level deeper, under `mcp.servers`. Use the shape
above unless your version rejects it.

### Kilo Code

The Kilo CLI and the VS Code extension share `~/.config/kilo/kilo.jsonc`, in the same shape
as opencode:

```json
{
  "mcp": {
    "arin": { "type": "local", "command": ["arin", "mcp"], "enabled": true }
  }
}
```

In the extension, Settings, then MCP, then Add Server with Local (stdio) writes the same
entry. Older releases kept servers in `mcp_settings.json`, in the standard JSON above.

### pi

MCP is built into pi from 0.99.0.

```sh
pi mcp add arin --exposure direct -- arin mcp
pi mcp list
```

That writes `~/.pi/agent/mcp.json`. Pass `--exposure direct`. Without it pi hands MCP tools
to its codemode sandbox instead of to the model, which then has to search for them before it
can point at anything. Pi also does not pass the instructions Arin sends at startup on to
the model, so what it knows about aiming comes from the tool descriptions alone.

### Ollama

Ollama runs the model and is not an MCP client itself, so there is nothing to set up in it.
Set Arin up in the agent instead, and point that agent at Ollama: `ollama launch codex`,
`ollama launch opencode`, `ollama launch pi` and `ollama launch claude` each start one with
Ollama as the provider, and each picks Arin up from its own configuration above.

The model is what decides how well that works. It needs tool calling to reach Arin at all,
and vision to aim off a screenshot. One without vision can still point at a named position
like `"top-right"`, which gets near the target but not onto it.

## The tools

Named after what an agent is trying to do rather than after the message underneath, so a
model reaches for the right one without being told.

| Tool | What it does |
|---|---|
| `point_at` | Puts the orb on a position, with an optional caption |
| `highlight` | Outlines a region, with an optional caption |
| `annotate` | Places a block of explanatory text, as a quiet note or a louder guide |
| `draw_arrow` | Draws an arrow between two places, curved unless asked straight |
| `clear` | Removes one mark, or every mark the agent drew |
| `bring_to_front` | Raises an application, when the daemon allows activation |
| `wait_until_showing` | Waits until the user has an application's window in front of them |

Every call reports back the display's size and scale, so an agent working from a
screenshot can convert pixels to logical points without asking twice.

## How an agent aims

Nothing above can see your screen. There is no `capture` tool and no `displays` tool, on
purpose: Arin holds the Screen Recording grant and a client does not, so letting a client
read the screen through Arin is the one capability worth gating. That leaves three ways to
say where, and they cost different amounts to set up.

| Form | Needs | Precision |
|---|---|---|
| `at` as percentages, `"27%,9%"` | a screenshot of one whole display | exact, to your measurement |
| `at` as a name, `"top-right"` | nothing | a region of the screen, not a spot |
| `x` and `y` | the display's logical size | exact |
| `query`, `"the Submit button"` | a resolver you configured | whatever the model gets right |

**The percentage form is the one most agents should reach for.** An agent holding a
screenshot can measure the target as a fraction of the image and send that, with no idea
how big the display is and no resolver configured. Both sides need the sign, so `"27,9"` is
refused rather than read as points. It aims `point_at` and either end of `draw_arrow`: a
name is a spot rather than an area, so `highlight` has to be measured.

Aim at the display the screenshot came from. A capture of one screen on a machine with
three does not describe the others, and a percentage sent to the wrong one lands somewhere
confidently wrong.

The server tells the agent all of this itself, in the instructions it sends at startup, so
there is nothing to paste into a prompt. [Resolvers](/docs/resolvers/) covers `query`,
which is off until you name one.

Marks live until they are cleared, the content scrolls out from under them, or the client
disconnects. Pass `ttl_seconds` to have one remove itself instead, counted from when it
finishes drawing.

Paths, arrows and sketched highlights are drawn on with the orb as the pen, so a tool
result can carry `starts_in_ms` and `reveal_ms`: how long until the pen starts on this
mark, and how long it takes. An agent speaking alongside the drawing can use the sum to
say "this one" as the mark lands.

When a mark goes away for a reason the agent did not ask for, that arrives as a `gone`
field on the next tool result. There is no way for an MCP server to interrupt a model
mid-thought, so the news waits for the next exchange. A daemon restart shows up the same
way, as one `session_end` with no annotation id, since every mark went with it.
