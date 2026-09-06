# The Homebrew cask (task 8.3).
#
# This file is the source of truth; `just cask` regenerates it from the built
# artifacts so the version and checksum can never be typed in wrong, and the
# result is copied into the tap at zaghaghi/homebrew-tap.
#
# `just cask-check` runs `brew readall` over it, not only `brew style`. Style is
# RuboCop and passes a cask Homebrew then refuses to load: it said "no offenses"
# about a `depends_on macos: :catalina` that `brew tap` rejected outright.
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
  version "1.1.0"
  # Replaced by `just cask <version>`, which reads it from the release's
  # SHA256SUMS. Left as an impossible value rather than :no_check: a cask
  # published by accident must fail to install, not install anything.
  sha256 "5f39ff5d3ba634f728b3ab3c548848e052e7ffcc5a502cff80b36ed1f88c93f9"

  url "https://github.com/zaghaghi/toolog/releases/download/v#{version}/toolog_#{version}_universal.dmg"
  name "toolog"
  desc "Local audit trail for Claude Code tool calls"
  homepage "https://github.com/zaghaghi/toolog"

  livecheck do
    url :url
    strategy :github_latest
  end

  # The floor, now that it is one Homebrew can express.
  #
  # v1.1 shipped `depends_on :macos` with no version, and said why: the bundle's
  # floor was 10.15, `:catalina` is no longer in Homebrew's version table, and
  # naming any version we did support would have claimed a *higher* floor than
  # the app had. That comment ended by predicting this change — "if the floor
  # ever moves to 11.0, which a local inference model would force" — and Phase
  # 13 is that model. llama.cpp's C++ needs `std::filesystem`.
  #
  # 11.0 is now true in three places that must agree, and each is checked:
  # `tauri.conf.json`'s `minimumSystemVersion` (asserted in `tests/bundle.rs`),
  # the built binary's `LC_BUILD_VERSION` (asserted in `just verify-bundle`),
  # and this line — which is the only one a user meets before downloading
  # anything, and so the only one that can refuse rather than fail.
  # `:big_sur` bare, not `">= :big_sur"`: Homebrew reads the symbol form as "this
  # version or newer" and deprecates the string comparison — `brew style`
  # rejected the string form outright, which is exactly what `just cask-check`
  # exists to catch before a tap does.
  depends_on macos: :big_sur

  app "toolog.app"
  binary "#{appdir}/toolog.app/Contents/MacOS/toolog"

  # Written in Homebrew's canonical order, which is also the order it runs them
  # in whatever order they appear: unload the agent, quit the app, then let the
  # app's own uninstaller put ~/.claude/settings.json back. That sequence is the
  # right one — the script must not run while a resident process still holds the
  # database and the port.
  uninstall launchctl: "com.zaghaghi.toolog",
            quit:      "com.zaghaghi.toolog",
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
