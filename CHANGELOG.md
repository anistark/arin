# Changelog

All notable changes to this project are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Minors
ship biweekly. 1.0 is the protocol freeze, after which protocol changes are additive
only.

## [Unreleased]

### Added

- `arin-protocol`: the 0.1 wire protocol as types plus validation, with no IO. Message
  envelope and version negotiation, logical geometry, anchors, opaque identifiers, and
  golden tests pinned to the spec examples.
- `arin-core`: the daemon. Unix socket server on a 0600 socket with a peer credential
  check and a 1MB payload cap, the session and annotation state machine, the `Renderer`,
  `Capture`, and `Resolver` seams, the scroll watcher, and a socket client shared by the
  CLI and the MCP server.
- `arin-cli`: the `arin` binary, with `daemon`, `point`, `highlight`, `clear`, and
  `status`. `daemon --headless` runs the whole protocol with no renderer.
- `arin-resolve`: the resolver registry. No adapters yet, those land in 0.3.
- `arin-mac`, `arin-linux`, `arin-win`, `arin-mcp`: crate scaffolds carrying their
  documented scope. The macOS backend returns errors rather than panicking, so the daemon
  runs end to end and fails at the point where drawing would happen.
- CI covering the two invariants the architecture rests on: core and the protocol build
  and test on Linux with no platform crate in the tree, and no input synthesis API is
  referenced anywhere in the source.
- A `justfile` for the common tasks, including a `ci` recipe that mirrors what CI runs.

### Notes

Nothing is drawn on screen yet. The macOS renderer is the remaining 0.1 work, and the
roadmap tracks it.

[Unreleased]: https://github.com/your-org/arin/compare/HEAD
