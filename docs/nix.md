# Nix

The second way to install Arin on a Mac, after the Homebrew tap. It needs
[Nix](https://nixos.org/download/) with flakes enabled, and nothing else: no Rust toolchain,
no Xcode.

```sh
nix run github:anistark/arin -- -d
```

That builds Arin and starts the daemon. Bare `nix run github:anistark/arin` prints help,
because bare `arin` prints help everywhere else and a command that reads like a question
should not leave a daemon running behind it.

To keep it:

```sh
nix profile install github:anistark/arin
```

macOS 14 or newer, on Apple silicon or Intel. There is no Linux package, and that is not an
oversight: the Linux renderer is v2, so the daemon would speak the whole protocol and draw
nothing.

## What gets installed

`Arin.app`, not a bare binary. macOS keys the Screen Recording grant to a bundle, the menu
bar item and the absent Dock icon are both properties of `Info.plist`, and the app
recognises itself when Finder opens it by the identifier LaunchServices hands it. A package
that installed `bin/arin` alone would build the same code and be a different product.

```
result/
  Applications/Arin.app      the bundle, with launch-agent.sh in its Resources
  bin/arin                   a symlink into the bundle, so PATH and the app are one binary
```

Installed into a profile or through `environment.systemPackages`, the app is linked into
`/Applications/Nix Apps/`, which is where Spotlight will find it.

## Starting it at login

The flake ships a [nix-darwin](https://github.com/nix-darwin/nix-darwin) module.

```nix
{
  inputs.arin.url = "github:anistark/arin";

  # in your darwinConfiguration
  modules = [
    arin.darwinModules.default
    {
      services.arin = {
        enable = true;
        resolver = "local";
      };
    }
  ];
}
```

That writes `~/Library/LaunchAgents/com.anistark.arin.plist` and starts the daemon at
login, from the binary inside the bundle. It is the same launch agent
`launch-agent.sh enable` installs, under the same label, so the two collide rather than
running two daemons against one socket. Pick one. If you have run the script before,
`nix-darwin` will refuse to overwrite the file it left behind and tell you to move it out
of the way.

nix-darwin needs `system.primaryUser` set to own a user agent.

### Options

Every option below maps to a flag on `arin daemon`, and leaving one unset leaves the
daemon's own default in place. `arin daemon --help` is the fuller explanation of what each
one does.

| Option | Type | Default | What it does |
|---|---|---|---|
| `services.arin.enable` | bool | `false` | Start the daemon at login |
| `services.arin.package` | package | this flake's | Has to be one containing `Arin.app` |
| `services.arin.socket` | path or null | `null` | Where the daemon listens |
| `services.arin.headless` | bool | `false` | Run the protocol and draw nothing |
| `services.arin.resolver` | string or null | `null` | Ground natural language targets with this resolver |
| `services.arin.groundingConsent` | `ask`, `always`, `never`, or null | `null` | Whether a client may make Arin look at the screen |
| `services.arin.color` | string or null | `null` | Preferred mark colour, as `#RRGGBB` |
| `services.arin.palette` | list of string | `[ ]` | The full set of colours to choose between |
| `services.arin.adaptiveColor` | bool | `true` | Look at the screen to pick a visible colour |
| `services.arin.checkUpdates` | bool | `false` | Ask GitHub once a day for a newer version |
| `services.arin.extraFlags` | list of string | `[ ]` | Anything the module does not name yet |
| `services.arin.logFile` | string or null | `/tmp/arin.log` | Where the agent's output goes |

`logFile` has to be absolute, in a directory that already exists. launchd does not expand
`~` and will not create a parent, and a Nix module cannot know your home directory while it
is being evaluated, so the default is somewhere that always exists rather than somewhere
good.

Naming a resolver is consent to the daemon reading your screen when a client asks it to.
`local` sends nothing off the machine. `claude` uploads a screenshot per query. Arin asks
before the first one either way, unless you set `groundingConsent = "always"`, which is
what an unattended daemon wants and is worth writing down deliberately rather than
arriving at.

## The overlay

For pulling Arin into your own package set:

```nix
nixpkgs.overlays = [ arin.overlays.default ];  # gives you pkgs.arin
```

## Permissions, and what an update costs

Arin asks for Screen Recording on first run. It is the only permission it ever asks for,
and it needs it for two things: noticing when content moves under a mark, and picking a
colour that can be seen against whatever is underneath.

**Today, every update asks again.** Nothing is signed yet, and macOS identifies unsigned
code by its hash and its path, both of which change on every Nix build. This is not
peculiar to Nix, `brew upgrade` has the same problem, but a store path makes it visible.
Signing lands in 0.7, after which the grant follows the certificate and the bundle
identifier rather than the path, and updating stops costing anything.

If the daemon keeps asking after you have granted it, two builds are competing for one row
in System Settings:

```sh
tccutil reset ScreenCapture com.anistark.arin
```

Then start whichever one you meant to run.

## Building on it

```sh
nix develop      # the toolchain and the tools the justfile reaches for
nix flake check  # builds the package
nix fmt          # nixfmt
```

Building the package does not run the test suite. CI runs it on every pull request against
the same code, and making every person who installs Arin run it again is most of the
difference between a slow install and a quick one.

If you would rather pay the minutes than take CI's word for it, there is a second package
that runs the whole workspace suite inside the build:

```sh
nix run github:anistark/arin#arin-tested -- -d
nix build github:anistark/arin#arin-tested
```

It is the same Arin. `doCheck` is part of a derivation, so it compiles from scratch rather
than reusing the untested build, and it is worth it mainly when you are installing from a
branch or a commit CI has never seen. `nix flake check` builds both.

The dev shell is offered rather than required. Arin is developed here with rustup and the
system Xcode, and what the shell has to stay is one in which `just ci` passes.

## Intel

`x86_64-darwin` is exposed and evaluated in CI, and it is not built there: GitHub's Intel
runners are being retired and nixpkgs 26.05 is the last release that supports the platform
at all. It should work. Nobody has watched it.
