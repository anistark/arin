<div align="center">

<img src="assets/logo.png" width="150" alt="Arin">

# ARIN

**An annotation layer any agent can draw on.**

Arin is a local daemon that draws on your screen. Point at things, highlight regions,
write explanations. Any AI can drive it over MCP or a small JSON protocol.

It draws. It never clicks.

[![CI](https://github.com/anistark/arin/actions/workflows/ci.yml/badge.svg)](https://github.com/anistark/arin/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/arin.svg?logo=rust&color=E5B45B)](https://crates.io/crates/arin)
[![docs.rs](https://img.shields.io/docsrs/arin?logo=docsdotrs&label=docs.rs)](https://docs.rs/arin)
[![License: MIT](https://img.shields.io/badge/license-MIT-6E7DDB.svg)](LICENSE)

[![macOS](https://img.shields.io/badge/macOS-14%2B-000000?logo=apple&logoColor=white)](https://github.com/anistark/arin/releases/latest)
[![Homebrew](https://img.shields.io/badge/brew-anistark%2Ftools%2Farin-FBB040?logo=homebrew&logoColor=white)](https://github.com/anistark/homebrew-tools)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-CE422B?logo=rust&logoColor=white)](https://www.rust-lang.org)
[![MCP](https://img.shields.io/badge/MCP-server-6E7DDB)](https://anistark.github.io/arin/docs/mcp/)

```sh
brew install anistark/tools/arin
```

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
