# Arin, packaged for Nix, as the Arin.app bundle rather than as a bare binary.
#
# The bundle is not decoration. macOS keys the Screen Recording grant to a bundle identity,
# the menu bar item and the absent Dock icon are both properties of Info.plist, and
# `Launch::detect` recognises its own bundle by the identifier LaunchServices puts in the
# environment. A package that installed `bin/arin` alone would build the same code and be a
# different product, so this mirrors what packaging/macos/bundle.sh assembles.
#
# It is not that script. `bundle.sh` shells out to `sips`, `iconutil`, `lipo` and `rustup`,
# none of which a Nix build may assume, so the assembly is written twice on purpose and the
# two are held together by using the same Info.plist, the same launch agent template, and
# the same logo. What differs, deliberately:
#
#   - One architecture, not a universal binary. Nix builds the package for the system it is
#     evaluated on, so a universal slice would mean cross compiling to produce half an
#     output nobody on this machine will run.
#   - The icon is built with imagemagick and libicns instead of sips and iconutil, and
#     carries no @2x entries. The sizes below are the ones png2icns maps to an icns type.
{
  lib,
  rustPlatform,
  apple-sdk_15,
  darwinMinVersionHook,
  imagemagick,
  libicns,
}:

let
  workspace = (lib.importTOML ../../Cargo.toml).workspace.package;

  # Only what the build reads. Everything else in the repo, the docs site above all, would
  # otherwise put a rebuild behind a typo fix.
  source = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      ../../Cargo.toml
      ../../Cargo.lock
      ../../crates
      ../../assets/logo.png
      # arin-cli include_str!s Info.plist to check the identifier has not drifted, so this
      # is a build input and not only an installation one.
      ../../packaging/macos
    ];
  };
in
rustPlatform.buildRustPackage {
  pname = "arin";
  inherit (workspace) version;

  src = source;
  cargoLock.lockFile = ../../Cargo.lock;

  nativeBuildInputs = [
    imagemagick
    libicns
  ];

  # ScreenCaptureKit's SCScreenshotManager is 14.0, and capture is how colours are chosen
  # and how scrolls are followed, so it is not a feature that can be compiled out. The
  # deployment target is stated for the same reason Info.plist states LSMinimumSystemVersion:
  # a build that links against a newer SDK while claiming an older floor fails at runtime on
  # the machine that believed the claim.
  buildInputs = [
    apple-sdk_15
    (darwinMinVersionHook "14.0")
  ];

  # One binary in the workspace, and the other crates are libraries it pulls in. Naming it
  # keeps the build off arin-linux and arin-win, which are empty scaffolds for v2.
  cargoBuildFlags = [
    "--package"
    "arin-cli"
  ];

  # The whole suite, the same set `just test` and CI run. It is headless by design: platform
  # behaviour arrives through traits and the tests wire up fakes, so nothing here wants a
  # window server.
  cargoTestFlags = [ "--workspace" ];

  postInstall = ''
    app=$out/Applications/Arin.app
    contents=$app/Contents
    mkdir -p "$contents/MacOS" "$contents/Resources"

    for size in 16 32 48 128 256 512; do
      magick assets/logo.png -resize "''${size}x''${size}" "icon_$size.png"
    done
    png2icns "$contents/Resources/AppIcon.icns" \
      icon_16.png icon_32.png icon_48.png icon_128.png icon_256.png icon_512.png

    substitute packaging/macos/Info.plist "$contents/Info.plist" \
      --replace-fail @VERSION@ "$version"

    # The binary lives inside the bundle and PATH gets a link to it, which is how the
    # Homebrew formula does it too. A second copy on PATH would be a second identity
    # asking for the same Screen Recording grant, and the one the user granted would not
    # be the one their agent started.
    mv "$out/bin/arin" "$contents/MacOS/arin"
    ln -s "$contents/MacOS/arin" "$out/bin/arin"

    # The launch agent installer travels with the app, because the person who needs it has
    # an installed app and not a clone. It finds its template next to itself, so the two
    # have to stay together. Nix users have services.arin in the nix-darwin module and
    # should prefer it, but an app that only starts at login when your configuration is
    # written in Nix would be a worse app.
    install -m555 packaging/macos/launch-agent.sh "$contents/Resources/launch-agent.sh"
    install -m444 packaging/macos/com.anistark.arin.plist "$contents/Resources/"

    printf 'APPL????' > "$contents/PkgInfo"
  '';

  # Both halves are checked because both have failed elsewhere: a bundle whose Info.plist
  # still says @VERSION@ installs and then reports nothing useful about itself, and a
  # `bin/arin` that stopped pointing into the bundle is the exact shape of the permission
  # bug the symlink exists to avoid.
  doInstallCheck = true;
  installCheckPhase = ''
    runHook preInstallCheck

    plist=$out/Applications/Arin.app/Contents/Info.plist
    grep -q "<string>$version</string>" "$plist" || {
      echo "Info.plist does not carry version $version" >&2
      exit 1
    }
    [ "$(readlink "$out/bin/arin")" = "$out/Applications/Arin.app/Contents/MacOS/arin" ] || {
      echo "bin/arin does not point into the bundle" >&2
      exit 1
    }
    "$out/bin/arin" --version | grep -q "arin $version"

    runHook postInstallCheck
  '';

  meta = {
    inherit (workspace) description;
    homepage = workspace.repository;
    changelog = "${workspace.repository}/blob/v${workspace.version}/CHANGELOG.md";
    license = lib.licenses.mit;
    mainProgram = "arin";
    # v1 is a Mac app. The Linux and Windows renderers are v2, and until they exist a
    # package for either would install a daemon that draws nothing.
    platforms = lib.platforms.darwin;
  };
}
