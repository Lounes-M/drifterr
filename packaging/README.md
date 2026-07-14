# Packaging & distribution

Install-via-package-manager templates for Drifterr. All are **versioned
templates**: on each release, bump the version and fill the checksums, then
publish/submit.

```
packaging/
  checksums.sh            # print SHA256 for the release DMG + EXE
  homebrew/drifterr.rb    # Homebrew cask (macOS)  → brew install --cask drifterr
  winget/                 # winget manifest (Windows) → winget install Drifterr.Drifterr
```

## Per-release steps

1. Cut and publish the GitHub release (see `../RELEASING.md`).
2. Get the checksums:

   ```bash
   packaging/checksums.sh 0.2.3
   ```

3. Update the versions + checksums:
   - `homebrew/drifterr.rb` — `version` and `sha256`.
   - `winget/*.yaml` — `PackageVersion` in all three, and `InstallerSha256`
     + `InstallerUrl` in the installer manifest.

## Homebrew (macOS)

See `homebrew/README.md`. Install from the file directly, or publish a tap
(`Lounes-M/homebrew-drifterr` with `Casks/drifterr.rb`) to offer
`brew install --cask drifterr`.

## winget (Windows)

The three files in `winget/` are a complete manifest for
`microsoft/winget-pkgs`. To submit:

1. Validate locally: `winget validate --manifest packaging/winget`
2. (Optional) test install: `winget install --manifest packaging/winget`
3. Open a PR to `microsoft/winget-pkgs` placing the files under
   `manifests/d/Drifterr/Drifterr/<version>/`.

Once accepted: `winget install Drifterr.Drifterr`.

> Note: the app ships unsigned today, so SmartScreen may warn on first run
> until code signing is set up (see `../RELEASING.md`).
