<div align="center">

<img src="assets/logo.png" width="150" alt="Arin">

# ARIN

**An annotation layer any agent can draw on.**

Arin is a local daemon that draws on your screen. Point at things, highlight regions,
write explanations. Any AI can drive it over MCP or a small JSON protocol.

It draws. It never clicks.

[Website](https://anistark.github.io/arin/) · [Docs](https://anistark.github.io/arin/docs/)

</div>

---

## What it does

An agent explaining an article can circle each section as it talks about it. An agent
reviewing your code can point at the line it means. A tutorial agent can show you where
a button is instead of describing it.

Arin holds no model and makes no decisions. The intelligence is your agent. Arin is the
chalk.

## Why draw only

Arin never synthesises input. No clicks, no keystrokes, no scrolling. That boundary is
the point: a tool that can only render pixels is safe to leave running, needs no
Accessibility permission, and is auditable in an afternoon. Agents that need to act
should compose Arin with a separate actuator.

## Privacy

TBD.

## License

MIT
