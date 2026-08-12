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

**Building rather than downloading is the point, not a shortcut.** There is no certificate
yet, and macOS quarantines anything unsigned that arrived over the network, so a downloaded
app gets refused by Gatekeeper. Something compiled where it runs was never downloaded,
carries no quarantine attribute, and Gatekeeper never engages. A certificate lands in 0.7,
and the formula is replaced by a cask then.

The bundle is still ad-hoc signed, which buys nothing from Gatekeeper and is not optional
anyway: macOS will not remember a Screen Recording grant for a bundle whose signature does
not verify.

It installs `Arin.app` into the Homebrew prefix and links the binary inside it onto your
PATH, so `arin` and the app are one file rather than two that could drift.

```sh
brew upgrade arin       # a newer version
brew uninstall arin     # and gone
```

Two consequences of the app living in the Homebrew prefix rather than `/Applications`:
Spotlight will not find it, and every upgrade asks for Screen Recording again. Both are
things a certificate fixes. Without one, the grant is pinned to the exact binary it was made
against, so replacing that binary voids it. A build signed with a Developer ID satisfies the
same requirement as the one before it, which is what lets a grant outlive an upgrade.

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

```sh
arin service enable
```

The same line however Arin got here. It works out which `Arin.app` to run from the binary
you typed it with, so there is no path to get right and no way to point the agent at a
different build of Arin than the one you meant. Re-running it replaces the agent, which is
how you point it at an app that moved.

```sh
arin service status      # installed? running? which build?
arin service restart     # what to run after brew upgrade
arin service disable     # stop starting at login, leave the app alone
```

`status` exits non-zero when the agent is not installed, so a setup script can ask.

The agent names a path that survives an upgrade, so `brew upgrade arin` leaves it working.
What an upgrade does not do is replace a daemon that is already running, so the old build
stays up until `arin service restart` says otherwise.

If you have a bare binary from `cargo install`, there is no bundle for the agent to start
and Arin will say so rather than installing one. Screen Recording is granted to a bundle
rather than to a path, so an agent running a bare binary comes up unable to see the screen.
Build a bundle with `just bundle` and name it:

```sh
arin service enable --app target/bundle/Arin.app
```

With Nix, it is `services.arin.enable = true` in your nix-darwin configuration, which is
the same launch agent with the daemon's command line written down. See [Nix](/docs/nix/).
`arin service` knows about it and refuses to manage an agent nix-darwin is managing, rather
than overwriting it and leaving two definitions of one agent.

## When permissions go wrong

If the daemon keeps asking for Screen Recording after you have granted it, start here:

```sh
arin permissions
```

It reports the permission and, when something is wrong, the thing the permission cannot
tell you: whether this build has an identity a grant can attach to at all. macOS remembers
a grant against a code signature rather than against a name, so a build whose signature does
not verify reports exactly what a build nobody has granted reports. Only one of those is
fixed in System Settings, and switching the row on for the other does nothing however many
times you do it.

Two causes, and the command above tells them apart.

**The build is not signed properly.** Arin installed before this was fixed carries a
signature that does not verify. Re-signing it is enough, and any ad-hoc signature will do:

```sh
codesign --force --sign - /opt/homebrew/opt/arin/Arin.app
tccutil reset ScreenCapture com.anistark.arin
```

Then start Arin and grant it once more. A grant made this way holds until that build is
replaced, so an upgrade will ask again until releases are signed with a certificate.

**Two builds are competing for one row.** macOS identifies unsigned code per binary and
shows a single row for the identifier, so toggling it updates whichever record it reaches
and the other keeps asking. Reset the same way, then start the one you actually meant to
run. `arin diagnose` reports which build you are talking to.

## Uninstalling

If you enabled the login agent, disable it *first*. The command that does so is the app, so
removing the app first takes it with it and leaves an agent behind pointing at a binary
that is no longer there.

```sh
arin service disable
```

Then the app:

```sh
brew uninstall arin                  # Homebrew
nix profile remove arin              # Nix
rm -rf /Applications/Arin.app        # the dmg
```

None of those remove what Arin left in your home directory, because none of them should
guess. If you want it gone completely:

```sh
rm -rf ~/Library/Logs/Arin
tccutil reset ScreenCapture com.anistark.arin
```

If the app went first, there is no `arin` left to run and the two lines it would have run
are:

```sh
launchctl bootout gui/$UID/com.anistark.arin
rm -f ~/Library/LaunchAgents/com.anistark.arin.plist
```

The socket lives in your temporary directory and is recreated on every start, so there is
nothing to clean up there. There are no accounts, no config in `~/.config`, and no
telemetry, so there is nothing else.
