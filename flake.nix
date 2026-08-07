# Arin as a flake: the second way to install it on a Mac, after the Homebrew tap.
#
#   nix run github:anistark/arin -- -d        start the daemon
#   nix profile install github:anistark/arin  keep it, with Arin.app in /Applications/Nix Apps
#
# macOS only, and that is not an oversight. v1 is a Mac app: the Linux and Windows
# renderers are v2, so a `x86_64-linux` output here would build a daemon that speaks the
# whole protocol and draws nothing.
#
# There is no `url`/`sha256` pair to rewrite at release time, which is the one way this is
# simpler than the Homebrew formula. A flake reference names a git revision and Nix
# checksums what it fetched, so `packaging/nix/` can change on any commit without a release
# job having to keep a copy of it somewhere else in step.
{
  description = "An annotation layer any agent can draw on.";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
  };

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "aarch64-darwin"
        "x86_64-darwin"
      ];

      forEachSystem = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      packageFor = pkgs: pkgs.callPackage ./packaging/nix/package.nix { };
    in
    {
      packages = forEachSystem (pkgs: rec {
        arin = packageFor pkgs;
        default = arin;
      });

      # Bare `arin` prints help, here as everywhere else, so `nix run` on its own is a
      # look around and `nix run github:anistark/arin -- -d` is the daemon. Starting a
      # daemon because somebody typed `nix run` would leave a process holding a Screen
      # Recording grant behind a command that reads like a question.
      apps = forEachSystem (
        pkgs:
        let
          arin = {
            type = "app";
            program = nixpkgs.lib.getExe self.packages.${pkgs.stdenv.hostPlatform.system}.arin;
          };
        in
        {
          inherit arin;
          default = arin;
        }
      );

      overlays.default = final: _prev: { arin = packageFor final; };

      # `services.arin.enable = true` in a nix-darwin configuration. Takes `self` so the
      # daemon it starts is built from the same revision as the module describing it.
      darwinModules = rec {
        arin = import ./packaging/nix/darwin-module.nix { inherit self; };
        default = arin;
      };

      # The toolchain, and the tools the justfile reaches for. Not how Arin is developed
      # here, which is rustup plus the system Xcode, so this is offered rather than
      # required: what it has to stay is a shell in which `just ci` passes.
      devShells = forEachSystem (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            clippy
            rust-analyzer
            rustc
            rustfmt

            just
            nodejs
            pnpm
          ];

          buildInputs = [
            pkgs.apple-sdk_15
            (pkgs.darwinMinVersionHook "14.0")
          ];
        };
      });

      checks = forEachSystem (pkgs: {
        inherit (self.packages.${pkgs.stdenv.hostPlatform.system}) arin;
      });

      # The one output that is not macOS only. Formatting is not platform specific, and
      # core and the protocol are worked on from Linux, which is the whole point of the
      # rule that they build with no platform crate in the tree.
      formatter = nixpkgs.lib.genAttrs (
        systems
        ++ [
          "aarch64-linux"
          "x86_64-linux"
        ]
      ) (system: nixpkgs.legacyPackages.${system}.nixfmt);
    };
}
