cask "drifterr" do
  version "0.1.1"
  # The universal .dmg runs on both Apple silicon and Intel. Pin a real sha256
  # per release for a published cask; :no_check is fine for a personal tap.
  sha256 :no_check

  url "https://github.com/Lounes-M/drifterr/releases/download/v#{version}/Drifterr_#{version}_universal.dmg"
  name "Drifterr"
  desc "Local-first copilot that detects when an AI chat drifts from your intent"
  homepage "https://github.com/Lounes-M/drifterr"

  app "Drifterr.app"

  caveats <<~EOS
    Drifterr is not yet notarized, so on first launch macOS may warn you.
    Right-click the app and choose Open to run it the first time.
  EOS
end
