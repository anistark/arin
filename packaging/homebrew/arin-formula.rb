# Homebrew formula for Arin, building from source.
#
# The install path until signing exists. A downloaded app has to get past Gatekeeper, and
# getting past Gatekeeper needs a Developer ID certificate. Something compiled on the
# machine it runs on was never downloaded, carries no `com.apple.quarantine` attribute, and
# Gatekeeper never engages. So building from source is not a compromise here: it is the
# only route that installs cleanly before there is a certificate.
#
# Replaced by Casks/arin.rb once there is one, and this file deleted at the same time.
# Keeping both means one tap answering `brew install` and `brew install --cask` with two
# different builds of the same version, one signed and one not.
#
# This file is the source of truth. The `tap` job in .github/workflows/release.yml renders
# it into anistark/homebrew-tools as Formula/arin.rb and opens a pull request, so edit it
# here and never there: the copy in the tap is generated and will be overwritten.
#
# `url` and `sha256` below are rewritten at release time. The values checked in here are
# whatever the last release set, or placeholders before the first one, and this file is not
# installable as it stands in either case.
#
# ## Why the release, and not a push to main
#
# The install block and the tag have to move together. Syncing this file to the tap on
# every change to `packaging/` would eventually put a new install block next to an old
# `url`: rename `bundle.sh` here, and the tap would call the new name while still fetching
# the tarball that contains the old one. Every install breaks and nothing on main looks
# wrong. So the tag is the trigger, because the tag is what `url` names.
#
# ## The version is not the cycle number
#
# `url` names a git tag, Homebrew reads `version` out of that tag, and the `test` block
# below checks the binary agrees. So the tag has to equal `[workspace.package].version` in
# Cargo.toml, which is 0.2.1.
#
# It is tempting to tag the cycle instead, `v0.5.0` during the 0.5 cycle. That breaks three
# things at once: `brew test` fails because the binary still reports 0.2.1, the release
# workflow refuses to build because it checks the tag against the manifest, and
# `arin-protocol` would end up republished at a version that claims nothing about the wire
# format. Cycle numbers are planning, not releases.
#
# ## Bootstrapping the first one by hand
#
# The automation needs a release to fire on, and the first tap PR predates the first
# release. Until then, set the checksum manually:
#
#   curl -sL https://github.com/anistark/arin/archive/refs/tags/v0.2.1.tar.gz | shasum -a 256
#
# and confirm with `brew audit --strict --online anistark/tools/arin`, which fetches the url
# and checks the checksum rather than trusting what is written next to it.
class Arin < Formula
  desc "Annotation layer any agent can draw on"
  homepage "https://github.com/anistark/arin"
  url "https://github.com/anistark/arin/archive/refs/tags/v0.2.1.tar.gz"
  sha256 "REPLACE_WITH_TARBALL_SHA256"
  license "MIT"
  head "https://github.com/anistark/arin.git", branch: "main"

  depends_on "rust" => :build
  # ScreenCaptureKit's SCScreenshotManager is 14.0+, and capture is not optional: it is how
  # colours are chosen and how scrolls are followed.
  depends_on macos: :sonoma

  def install
    # The same script the release workflow runs, so the formula cannot drift from the dmg.
    # Unsigned deliberately: nothing here has a certificate, and an ad-hoc signature would
    # change on every build without buying anything Gatekeeper or TCC recognises.
    system "packaging/macos/bundle.sh", "--output", "target/bundle"

    # The bundle rather than a bare binary, because the menu bar item, the absence of a
    # Dock icon, and the Screen Recording grant are all properties of Info.plist.
    prefix.install "target/bundle/Arin.app"

    # One binary serving as the app and the command line tool, so this is a link into the
    # bundle rather than a second copy that could drift from it.
    bin.install_symlink prefix/"Arin.app/Contents/MacOS/arin"
  end

  def caveats
    <<~EOS
      Arin is a menu bar app with no Dock icon.

      Start it:

        arin -d

      Or start it at login:

        #{opt_prefix}/Arin.app/Contents/Resources/launch-agent.sh enable #{opt_prefix}/Arin.app

      Grant Screen Recording in System Settings > Privacy & Security when asked.

      This build is unsigned, which has one consequence worth knowing: macOS ties the
      Screen Recording grant to the exact binary, so upgrading Arin will ask for the
      permission again. A signed build keeps it across upgrades.

      The app lives in the Homebrew prefix rather than /Applications, so Spotlight will not
      find it. That arrives with the signed build, which ships as a cask.
    EOS
  end

  test do
    # Ties the tag in `url` to what the binary actually reports. If these ever disagree,
    # the tag was picked from something other than Cargo.toml.
    assert_match "arin #{version}", shell_output("#{bin}/arin --version")

    # The command surface exists without a window server, a display, or a permission grant,
    # which is the part a formula test can reach.
    assert_match "resolvers", shell_output("#{bin}/arin --help")
  end
end
