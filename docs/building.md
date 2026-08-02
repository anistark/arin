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

GitHub releases and a brew tap at `anistark/homebrew-tools` land at 0.5, making
`brew install anistark/tools/arin` the install line. That formula compiles from source on
your machine, which is deliberate: an unsigned app that was *downloaded* is quarantined and
refused by Gatekeeper, and one compiled locally is not.

Signing and notarization follow at 0.7, and the formula is replaced by a cask,
`brew install --cask anistark/tools/arin`. That is when installing stops requiring a
compile and the Screen Recording grant starts surviving upgrades.

Nix arrives at 0.6. Linux packages, `deb`, `rpm` and AUR, come with the Linux port rather
than before it. Until 0.5, building from source by hand is the only route.
