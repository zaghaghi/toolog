# The Homebrew cask (task 8.3).
#
# This file is the source of truth; `just cask` regenerates it from the built
# artifacts so the version and checksum can never be typed in wrong, and the
# result is copied into the tap at zaghaghi/homebrew-tap.
#
# Three stanzas here are not boilerplate and are the reason this file is worth
# reading:
#
#   binary   ADR-0007 ships one artifact that is both the menu-bar app and the
#            CLI, decided by argv. Without this, `toolog doctor` would not
#            exist on the PATH of someone who installed with brew, and half
#            the documented commands would be unreachable.
#
#   script   `brew uninstall` removes an application; it knows nothing about
#            the six variables this one added to ~/.claude/settings.json.
#            Running toolog's own uninstaller first is what makes the Phase 8
#            promise — that the file goes back byte for byte — true through
#            the package manager and not only through the terminal. It runs
#            without --delete-data, so removing the tool never destroys the
#            record it collected. `must_succeed: false` keeps a failed revert
#            from stranding a half-removed app.
#
#   zap      The other half of that choice: `brew uninstall --zap` is how
#            someone asks for the history to go too, and it maps onto exactly
#            what `toolog uninstall --delete-data` would have removed.
cask "toolog" do
  version "1.0.0"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"

  url "https://github.com/zaghaghi/toolog/releases/download/v#{version}/toolog_#{version}_universal.dmg",
      verified: "github.com/zaghaghi/toolog/"
  name "toolog"
  desc "Local audit trail for Claude Code tool calls"
  homepage "https://github.com/zaghaghi/toolog"

  livecheck do
    url :url
    strategy :github_latest
  end

  # Matches LSMinimumSystemVersion in the bundle. The artifact is universal, so
  # there is one download for both architectures.
  depends_on macos: ">= :catalina"

  app "toolog.app"
  binary "#{appdir}/toolog.app/Contents/MacOS/toolog"

  uninstall quit:      "com.zaghaghi.toolog",
            launchctl: "com.zaghaghi.toolog",
            script:    {
              executable:   "#{appdir}/toolog.app/Contents/MacOS/toolog",
              args:         ["uninstall", "--apply"],
              must_succeed: false,
            }

  zap trash: [
    "~/Library/Application Support/toolog",
    "~/Library/LaunchAgents/com.zaghaghi.toolog.plist",
  ]

  caveats <<~EOS
    toolog records what Claude Code does on this machine, and nothing leaves it.

    To finish setting up:

      toolog doctor --fix     configure Claude Code's telemetry (writes ~/.claude/settings.json,
                              merged, with a timestamped backup)
      toolog backfill         import the history you already have
      toolog                  start the menu-bar app

    To remove it later, `brew uninstall --cask toolog` also puts
    ~/.claude/settings.json back and leaves your recorded history alone. Add
    --zap to delete that too.
  EOS
end
