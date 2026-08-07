# Building Arin

Nothing is published yet. Until it is, Arin is built from source.

Needs a [Rust toolchain](https://rustup.rs). macOS 14.2 or newer.

```sh
git clone https://github.com/anistark/arin && cd arin
cargo install --path crates/arin-cli
```

That installs one binary, `arin`. It is the daemon, the scripting client, and the MCP
server, chosen by subcommand.

## Running the daemon

```sh
arin daemon
```

It asks for Screen Recording on first run. Arin needs it for two things: noticing when
content moves under a mark, and picking a colour that can be seen against whatever is
underneath. Those frames are compared in memory and are not written anywhere or sent
anywhere. It is the only permission Arin asks for, and in particular it never asks for
Accessibility.

Nothing is drawn until an agent asks.

On a platform with no renderer yet, `arin daemon --headless` runs the whole protocol and
draws nothing, which is how the daemon is exercised before a platform backend exists.

## Distribution

Three stages, in this order. No dates, because each one depends on the last.

**Now: build it yourself,** with the steps above.

**Next: a brew tap** at `anistark/homebrew-tools`, making
`brew install anistark/tools/arin` the install line. That formula still compiles from
source on your machine, which is deliberate rather than lazy: an unsigned app that was
*downloaded* is quarantined and refused by Gatekeeper, and one compiled locally is not. So
building from source is the only route that installs cleanly before there is a signing
certificate, not merely the cheaper one.

**Then: signed and notarized,** at which point the formula is replaced by a cask,
`brew install --cask anistark/tools/arin`, and the formula is removed so there is only ever
one build to install. This is when installing stops needing a compile, when the app can
live in `/Applications` where Spotlight will find it, and when the Screen Recording grant
starts surviving upgrades. Unsigned code is identified by its hash, so today every new
build asks for that permission again.

**Also now: Nix.** A flake for `aarch64-darwin` and `x86_64-darwin`, with a nix-darwin
module for the launch agent. `nix run github:anistark/arin -- -d` builds the same app
bundle and starts it, on a machine with no Rust toolchain on it. See
[Nix](https://anistark.github.io/arin/docs/nix/).

Linux packages, `deb`, `rpm` and AUR, come with the Linux port rather than before it.
