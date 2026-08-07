# Homebrew cask for Arin.
#
# This file lives here so it is versioned next to what it packages, and is copied into
# anistark/homebrew-tools as Casks/arin.rb on release. See plan/homebrew-tools-update.md.
#
# A cask rather than a formula, because the deliverable is an .app bundle. The formula path
# would install a bare binary, which is exactly the thing that cannot hold a Screen
# Recording grant across an update: TCC remembers a signed bundle identity, not a path.
#
# `version` and `sha256` are rewritten per release. Everything else should not move.
cask "arin" do
  version "0.3.0"
  sha256 "REPLACE_WITH_RELEASE_SHA256"

  url "https://github.com/anistark/arin/releases/download/v#{version}/Arin-#{version}-universal.dmg"
  name "Arin"
  desc "Annotation layer any agent can draw on"
  homepage "https://github.com/anistark/arin"

  # ScreenCaptureKit's SCScreenshotManager is 14.0+, and capture is not optional: it is how
  # colours are chosen and how scrolls are followed.
  depends_on macos: ">= :sonoma"

  app "Arin.app"

  # The CLI and the app are one binary, so this points inside the bundle rather than
  # shipping a second copy that could drift from it. `arin point`, `arin mcp` and the rest
  # are then on PATH, and they are the same build the menu bar is running.
  binary "#{appdir}/Arin.app/Contents/MacOS/arin", target: "arin"

  uninstall quit:       "com.anistark.arin",
            launchctl:  "com.anistark.arin",
            delete:     "#{appdir}/Arin.app"

  # The socket lives in the user's temporary directory and is recreated on every start, so
  # it is not listed here. What is listed is what a user would otherwise be left to find:
  # the log directory the launch agent writes to, and the agent itself if they enabled it.
  zap trash: [
    "~/Library/Logs/Arin",
    "~/Library/LaunchAgents/com.anistark.arin.plist",
  ]

  caveats <<~EOS
    Arin is a menu bar app with no Dock icon.

    Grant Screen Recording in System Settings > Privacy & Security > Screen Recording,
    then start it:

      arin -d

    To start it at login instead:

      /Applications/Arin.app/Contents/Resources/launch-agent.sh enable

    Grounding natural language targets is off unless you name a resolver, and it asks
    before it reads your screen. Nothing leaves the machine with --resolver local.
  EOS
end
