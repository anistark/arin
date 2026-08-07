# Install

macOS 14 or newer, on Apple silicon or Intel. There is no Linux or Windows build: the
renderers for both are v2, so the daemon would speak the whole protocol and draw nothing.

Four ways in. Homebrew is the one to use unless you have a reason.

## Homebrew

```sh
brew install anistark/tools/arin
```

That is a tap, `anistark/tools`, and the formula builds Arin from source on your machine.
It needs no Rust toolchain of your own: Homebrew installs one for the build and cleans up
after itself.

**Building rather than downloading is the point, not a shortcut.** Nothing is signed yet,
and macOS quarantines anything unsigned that arrived over the network, so a downloaded app
gets refused by Gatekeeper. Something compiled where it runs was never downloaded, carries
no quarantine attribute, and Gatekeeper never engages. Signing lands in 0.7, and the
formula is replaced by a cask then.

It installs `Arin.app` into the Homebrew prefix and links the binary inside it onto your
PATH, so `arin` and the app are one file rather than two that could drift.

```sh
brew upgrade arin       # a newer version
brew uninstall arin     # and gone
```

Two consequences of the app living in the Homebrew prefix rather than `/Applications`:
Spotlight will not find it, and every upgrade asks for Screen Recording again. Both are
things the signed build fixes.

## Nix

```sh
nix run github:anistark/arin -- -d
```

Also builds from source, also needs nothing installed beforehand, and additionally gives
you a nix-darwin module for starting at login with the daemon's options under version
control. [Nix](/docs/nix/) is the whole story, including `services.arin` and its options.

## The dmg

Every release attaches one, and the checksum is in the release notes.

**It is unsigned.** macOS will refuse it the first time and you will have to allow it in
System Settings under Privacy and Security. If that is not something you want to do, use
Homebrew, which avoids the question rather than answering it. This is the one install route
that gets easier rather than harder in 0.7.

## From source

```sh
git clone https://github.com/anistark/arin && cd arin
cargo install --path crates/arin-cli
```

Or without cloning:

```sh
cargo install --git https://github.com/anistark/arin arin-cli
```

Needs a [Rust toolchain](https://rustup.rs). This installs the bare binary and not the app
bundle, which is fine for driving the daemon and worse for living with: the menu bar item,
the absent Dock icon, and a Screen Recording grant that survives a rebuild are all
properties of the bundle. `just bundle` builds `Arin.app` from a clone if you want both.

`cargo install arin` does not work, on purpose. `arin` is a library on crates.io, so Cargo
correctly answers that there is nothing to install. The binary is `arin-cli`.

## First run

```sh
arin -d
```

macOS asks for Screen Recording. Arin needs it for two things: noticing when content moves
under a mark, and picking a colour that can be seen against whatever is underneath. Those
frames are compared in memory. They are not written anywhere and not sent anywhere.

It is the only permission Arin asks for. In particular it never asks for Accessibility,
which is the one that would let it act on your behalf, and it never will: Arin draws and
never clicks.

`arin -d` runs in the foreground and stops on Ctrl-C. That is deliberate. Backgrounding is
launchd's job, below.

## Starting at login

Not automatic, either way. An annotation daemon that added itself to your login items
unasked would be doing the thing people reasonably object to.

With Homebrew or the dmg:

```sh
$(brew --prefix)/opt/arin/Arin.app/Contents/Resources/launch-agent.sh enable
```

Or, from `/Applications` if you installed the dmg:

```sh
/Applications/Arin.app/Contents/Resources/launch-agent.sh enable
```

`disable` takes it away again and leaves the app alone. `status` says whether it is loaded.

With Nix, it is `services.arin.enable = true` in your nix-darwin configuration, which is
the same launch agent with the daemon's command line written down. See [Nix](/docs/nix/).

## Permissions, when they go wrong

If the daemon keeps asking for Screen Recording after you have granted it, there are two
builds of Arin on the machine competing for one row in System Settings. macOS identifies
unsigned code per binary and shows a single row for the identifier, so toggling it updates
whichever record it reaches and the other keeps asking.

```sh
tccutil reset ScreenCapture com.anistark.arin
```

Then start the one you actually meant to run. `arin diagnose` reports which build you are
talking to and whether it holds the permission.

## Uninstalling

```sh
brew uninstall arin                  # Homebrew
nix profile remove arin              # Nix
rm -rf /Applications/Arin.app        # the dmg
```

None of those remove what Arin left in your home directory, because none of them should
guess. If you want it gone completely:

```sh
launch-agent.sh disable              # if you enabled it
rm -rf ~/Library/Logs/Arin
tccutil reset ScreenCapture com.anistark.arin
```

The socket lives in your temporary directory and is recreated on every start, so there is
nothing to clean up there. There are no accounts, no config in `~/.config`, and no
telemetry, so there is nothing else.
