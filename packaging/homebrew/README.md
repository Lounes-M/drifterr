# Homebrew cask

`drifterr.rb` is a ready-to-use [Homebrew cask](https://docs.brew.sh/Cask-Cookbook)
that installs the universal macOS `.dmg` from the latest GitHub release.

Until the cask is accepted into `homebrew/cask` (or you publish a tap), install
it from this file directly:

```bash
brew install --cask ./packaging/homebrew/drifterr.rb
```

To offer the documented `brew install --cask drifterr`, publish a tap
(e.g. `Lounes-M/homebrew-drifterr`) containing `Casks/drifterr.rb`, then:

```bash
brew tap lounes-m/drifterr
brew install --cask drifterr
```

Bump `version` (and pin a real `sha256`) on each release.
