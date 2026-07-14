# Homebrew Cask for Drifterr (macOS). See ../README.md for how to fill sha256
# and publish. Install from a tap once published:  brew install --cask drifterr
cask "drifterr" do
  version "0.2.3"
  # Pin the DMG checksum per release (see ../checksums.sh). `:no_check` is fine
  # for a personal tap while unsigned; a real sha256 is required for homebrew/cask.
  sha256 :no_check

  url "https://github.com/Lounes-M/drifterr/releases/download/v#{version}/Drifterr_#{version}_universal.dmg",
      verified: "github.com/Lounes-M/drifterr/"
  name "Drifterr"
  desc "Local-first drift detector for AI chat sessions"
  homepage "https://drifterr.app/"

  # The app self-updates via tauri-plugin-updater; don't let brew fight it.
  auto_updates true
  depends_on macos: ">= :catalina"

  app "Drifterr.app"

  zap trash: [
    "~/Library/Application Support/com.drifterr.app",
    "~/Library/Caches/com.drifterr.app",
    "~/Library/Preferences/com.drifterr.app.plist",
  ]

  caveats <<~EOS
    Drifterr is currently distributed UNSIGNED. macOS Gatekeeper blocks the
    first launch — open it once with a right-click:

      Finder → Applications → right-click Drifterr → Open → Open

    or clear the quarantine flag:

      xattr -dr com.apple.quarantine "/Applications/Drifterr.app"

    Signed + notarized builds remove this step (see RELEASING.md).
  EOS
end
