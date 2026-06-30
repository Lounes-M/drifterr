# Releasing Drifterr

The desktop app (proxy + menubar fused into one binary) is built and published
by [`.github/workflows/release.yml`](.github/workflows/release.yml), triggered
by pushing a version tag. Installers are attached to a GitHub **release**, and
the landing page's Download button resolves the latest one per OS.

## One-time setup — updater signing secrets

Auto-update is enabled (`createUpdaterArtifacts: true`), so the build **must**
sign its updater artifacts. The public key already lives in
`apps/desktop/src-tauri/tauri.conf.json` (`plugins.updater.pubkey`). Add the
matching private key as repo secrets:

1. GitHub → repo → **Settings → Secrets and variables → Actions → New repository secret**.
2. Add:
   - `TAURI_SIGNING_PRIVATE_KEY` — the contents of the private key file.
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` — the key password (empty if none).

> If you ever rotate the key (`npx @tauri-apps/cli signer generate`), update
> **both** the `pubkey` in `tauri.conf.json` and these secrets, or existing
> installs won't accept updates.

macOS notarization / Windows code-signing are optional and **off for v0.1** —
the installers are unsigned, so users see a Gatekeeper / SmartScreen warning on
first launch (right-click → Open on macOS; "More info → Run anyway" on Windows).
The `APPLE_*` env block in the workflow is where notarization would be wired
later.

## Cut a release

```bash
# from an up-to-date main
git tag v0.1.0
git push origin v0.1.0
```

This runs the release workflow across macOS (Apple silicon + Intel), Linux and
Windows, and creates a **draft** release with:

- `Drifterr_<v>_universal.dmg` (macOS — one universal build, Apple silicon + Intel)
- `Drifterr_<v>_x64-setup.exe` (Windows, NSIS, per-user install — no admin)
- `Drifterr_<v>_amd64.AppImage`, `Drifterr_<v>_amd64.deb` (Linux)
- `latest.json` + `.sig` files (auto-update)

The macOS / Windows installers are **unsigned** for now, so the OS warns on
first launch (macOS: right-click → Open; Windows: More info → Run anyway). The
landing surfaces this tip via a toast when you click Download.

The version in the filenames comes from `version` in `tauri.conf.json` — bump it
there (and keep the tag in sync) for each release.

## Before publishing

1. Watch the four jobs in **Actions**; fix any compile/bundle errors and re-tag
   (`git tag -d v0.1.0 && git push origin :v0.1.0`, then re-create) if needed.
2. Download each installer from the draft and **launch it once** (the menubar
   opens, the embedded proxy starts on `:8787` / control on `:8788`).
3. Edit the draft release notes, then **Publish**.

## After publishing

- The landing **Download** button (`/download/<os>`) now serves the right
  installer automatically — no code change.
- Auto-update: subsequent releases are picked up by installed apps via
  `releases/latest/download/latest.json`.

## Asset naming ↔ download resolver

`apps/landing/api/download.js` picks the asset per OS by suffix
(`.dmg` / `-setup.exe` / `.AppImage`). If you change bundle `targets` in
`tauri.conf.json`, keep those matchers in sync.
