# The nix-darwin module: `services.arin.enable = true` and Arin starts at login.
#
# It is the same launch agent packaging/macos/com.anistark.arin.plist describes, written as
# options instead of as a template. The plist stays the reference: if the two ever say
# different things about KeepAlive or ProcessType, that file is right and this one is a
# drift to fix.
#
# What a Nix user gets that the script cannot give them is the daemon's command line under
# version control. `launch-agent.sh` installs a plist that runs `arin daemon` with no
# options, and anything past that is hand editing a generated file that the next `enable`
# overwrites.
#
# Takes the flake as an argument so `package` can default to the build from the same
# revision as the module. A module that defaulted to nixpkgs would silently pair this file
# with a different Arin.
{ self }:

{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.arin;
  inherit (lib) mkIf mkOption types;

  binary = "${cfg.package}/Applications/Arin.app/Contents/MacOS/arin";

  arguments =
    lib.optionals (cfg.socket != null) [
      "--socket"
      "${cfg.socket}"
    ]
    ++ [ "daemon" ]
    ++ lib.optional cfg.headless "--headless"
    ++ lib.optionals (cfg.resolver != null) [
      "--resolver"
      cfg.resolver
    ]
    ++ lib.optionals (cfg.groundingConsent != null) [
      "--grounding-consent"
      cfg.groundingConsent
    ]
    ++ lib.optionals (cfg.color != null) [
      "--color"
      cfg.color
    ]
    ++ lib.optionals (cfg.palette != [ ]) [
      "--palette"
      (lib.concatStringsSep "," cfg.palette)
    ]
    ++ lib.optional cfg.checkUpdates "--check-updates"
    ++ lib.optional (!cfg.adaptiveColor) "--no-adaptive-color"
    ++ cfg.extraFlags;
in
{
  options.services.arin = {
    enable = lib.mkEnableOption "the Arin annotation daemon, started at login";

    package = mkOption {
      type = types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.arin;
      defaultText = lib.literalExpression "arin.packages.\${system}.arin";
      description = ''
        The Arin package. It has to be one that contains `Arin.app`: the agent runs the
        binary inside the bundle, because macOS attributes the Screen Recording grant to
        the bundle and a bare binary would come up unable to see the screen.
      '';
    };

    socket = mkOption {
      type = types.nullOr types.path;
      default = null;
      example = "/tmp/arin.sock";
      description = ''
        Where the daemon listens, if not the default. Clients need the same path, through
        `--socket` or `ARIN_SOCKET`. Unix socket paths run out at around 104 bytes.
      '';
    };

    headless = mkOption {
      type = types.bool;
      default = false;
      description = ''
        Run the protocol and draw nothing. Useful for exercising a client, and not what
        you want from a login agent otherwise.
      '';
    };

    resolver = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "local";
      description = ''
        Ground natural language targets with this resolver. Off unless named, and naming
        one is consent to the daemon reading the screen when a client asks it to. `local`
        sends nothing off the machine. `claude` uploads a screenshot per query.
      '';
    };

    groundingConsent = mkOption {
      type = types.nullOr (
        types.enum [
          "ask"
          "always"
          "never"
        ]
      );
      default = null;
      description = ''
        Whether a client may make Arin look at the screen. `ask` prompts and remembers for
        as long as you said, and is the default the daemon applies when this is unset.
        `always` never prompts, which is what an unattended daemon wants and is worth
        writing down deliberately rather than arriving at.
      '';
    };

    color = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "#FFB020";
      description = ''
        The preferred mark colour, as `#RRGGBB`. Blue is refused: it belongs to the orb.
      '';
    };

    palette = mkOption {
      type = types.listOf types.str;
      default = [ ];
      example = [
        "#FFB020"
        "#FF3B30"
        "#30D158"
      ];
      description = ''
        The full set of colours to choose between, preferred first. Replaces the built-in
        palette rather than adding to it, and takes precedence over `color`.
      '';
    };

    adaptiveColor = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Look at the screen to pick a colour that can be seen against what is under the
        mark. Turning it off saves a capture per positioned annotation and costs the record
        of what each mark was drawn over, which is part of how a mark follows a scroll.
      '';
    };

    checkUpdates = mkOption {
      type = types.bool;
      default = false;
      description = ''
        Ask GitHub once a day whether there is a newer Arin, and say so in the menu bar. It
        only ever reports. On a Nix install the answer is not actionable from inside the
        app anyway, since updating means moving the flake input.
      '';
    };

    extraFlags = mkOption {
      type = types.listOf types.str;
      default = [ ];
      example = [ "--no-adaptive-color" ];
      description = ''
        Extra arguments for `arin daemon`, for anything this module does not name yet.
      '';
    };

    logFile = mkOption {
      type = types.nullOr types.str;
      default = "/tmp/arin.log";
      example = "/Users/you/Library/Logs/Arin/arin.log";
      description = ''
        Where the agent's output goes, or null to discard it. Absolute, and in a directory
        that already exists: launchd does not expand `~` and will not create a parent, and
        a Nix module cannot know your home directory while it is being evaluated. The
        default is somewhere that always exists rather than somewhere good.
      '';
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        assertion = pkgs.stdenv.hostPlatform.isDarwin;
        message = "services.arin needs macOS. The Linux renderer is v2.";
      }
    ];

    # Puts `arin` on PATH for clients and for the MCP config, and gets Arin.app linked into
    # /Applications/Nix Apps, which is what lets Spotlight find it.
    environment.systemPackages = [ cfg.package ];

    launchd.user.agents.arin = {
      serviceConfig = {
        # The identifier the app itself carries, rather than the org.nixos.* label
        # nix-darwin would otherwise generate. It is what `launchctl` commands in the docs
        # name, what the Homebrew cask removes on uninstall, and what TCC has on file.
        Label = "com.anistark.arin";

        ProgramArguments = [ binary ] ++ arguments;
        RunAtLoad = true;

        # Restart if it dies, but not if it dies immediately and repeatedly: a daemon that
        # cannot bind its socket should not be respawned into a loop that fills the log.
        KeepAlive.SuccessfulExit = false;
        ThrottleInterval = 10;

        ProcessType = "Interactive";
      }
      // lib.optionalAttrs (cfg.logFile != null) {
        StandardOutPath = cfg.logFile;
        StandardErrorPath = cfg.logFile;
      };
    };
  };
}
