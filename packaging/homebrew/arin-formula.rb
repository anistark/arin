# Homebrew formula for Arin, building from source.
#
# This is the 0.5 and 0.6 install path, and it exists because signing does not yet. A
# downloaded app has to get past Gatekeeper, and getting past Gatekeeper needs a Developer
# ID certificate, which arrives in 0.7. Something compiled on the machine it runs on was
# never downloaded, carries no `com.apple.quarantine` attribute, and Gatekeeper never
# engages. So building from source is not a compromise here: it is the only route that
# installs cleanly before there is a certificate.
#
# Replaced by Casks/arin.rb in 0.7. See plan/homebrew-tools-update.md.
#
# Copied into anistark/homebrew-tools as Formula/arin.rb.
class Arin < Formula
  desc "Annotation layer any agent can draw on"
  homepage "https://github.com/anistark/arin"
  url "https://github.com/anistark/arin/archive/refs/tags/v0.1.0.tar.gz"
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
      permission again. A signed build, from 0.7, keeps it across upgrades.

      The app lives in the Homebrew prefix rather than /Applications, so Spotlight will not
      find it. That also arrives with the signed build, which ships as a cask.
    EOS
  end

  test do
    assert_match "arin #{version}", shell_output("#{bin}/arin --version")

    # The protocol works with no renderer, which is the part that can be checked without a
    # window server, a display, or a permission grant.
    assert_match "resolvers", shell_output("#{bin}/arin --help")
  end
end
