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

Marks live until they are cleared, the content scrolls out from under them, or the client
disconnects. Pass `ttl_seconds` to have one remove itself instead.

When a mark goes away for a reason the agent did not ask for, that arrives as a `gone`
field on the next tool result. There is no way for an MCP server to interrupt a model
mid-thought, so the news waits for the next exchange.
