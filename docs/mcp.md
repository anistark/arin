# Using Arin from an agent

Arin speaks MCP over stdio. An MCP client launches `arin mcp` as a subprocess.

For Claude Code that is one line:

```sh
claude mcp add arin -- arin mcp
```

Or, in any client that takes the standard JSON:

```json
{
  "mcpServers": {
    "arin": { "command": "arin", "args": ["mcp"] }
  }
}
```

The daemon has to already be running. `arin mcp` connects to its socket and forwards.

## The tools

Four of them, named after what an agent is trying to do rather than after the message
underneath, so a model reaches for the right one without being told.

| Tool | What it does |
|---|---|
| `point_at` | Puts the orb on a position, with an optional caption |
| `highlight` | Outlines a region, with an optional caption |
| `annotate` | Places a block of explanatory text |
| `clear` | Removes one mark, or every mark the agent drew |

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
refused rather than read as points. It is on `point_at` only: a name is a spot rather than
an area, so `highlight` has to be measured.

Aim at the display the screenshot came from. A capture of one screen on a machine with
three does not describe the others, and a percentage sent to the wrong one lands somewhere
confidently wrong.

The server tells the agent all of this itself, in the instructions it sends at startup, so
there is nothing to paste into a prompt. [Resolvers](/docs/resolvers/) covers `query`,
which is off until you name one.

Marks live until they are cleared, the content scrolls out from under them, or the client
disconnects. Pass `ttl_seconds` to have one remove itself instead.

When a mark goes away for a reason the agent did not ask for, that arrives as a `gone`
field on the next tool result. There is no way for an MCP server to interrupt a model
mid-thought, so the news waits for the next exchange.
