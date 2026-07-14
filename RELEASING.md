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

## Code signing & notarization (removes the install warning)

Unsigned builds trigger a Gatekeeper (macOS) / SmartScreen (Windows) warning on
first launch. The workflow is **already wired** for both — it stays unsigned
until you add the secrets, then signs automatically with no workflow change.

### macOS — signed + notarized (highest impact; Gatekeeper is strict)

Prereq: an **Apple Developer account** ($99/yr) with a *Developer ID
Application* certificate.

1. In **Keychain Access**, export your "Developer ID Application" cert **and its
   private key** as a `.p12` (set a password). Base64-encode it:
   `base64 -i cert.p12 | pbcopy`.
2. Create an **app-specific password** at <https://appleid.apple.com> → Sign-In &
   Security → App-Specific Passwords.
3. Add these six repo secrets (**Settings → Secrets and variables → Actions**):
   | Secret | Value |
   |---|---|
   | `APPLE_CERTIFICATE` | the base64 `.p12` from step 1 |
   | `APPLE_CERTIFICATE_PASSWORD` | the `.p12` password |
   | `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Your Name (TEAMID)` |
   | `APPLE_ID` | your Apple ID email |
   | `APPLE_PASSWORD` | the app-specific password from step 2 |
   | `APPLE_TEAM_ID` | your 10-char Team ID |
4. Cut a release. tauri-action signs the universal `.app`/`.dmg` and notarizes it;
   the warning is gone. (First notarization can add a few minutes to the job.)

### Windows — SmartScreen

Unsigned `.exe` shows SmartScreen ("More info → Run anyway"). To remove it you
need a code-signing certificate (an OV/EV cert from a CA, or **Azure Trusted
Signing** — cheapest for indies). Two routes, both configured in
`tauri.conf.json` under `bundle.windows`:
- **Thumbprint**: import the cert on the runner, set `"certificateThumbprint"`.
- **Azure Trusted Signing**: set a `"signCommand"` that calls their signer.
Not wired yet (no CA cert); the SmartScreen tip on the download page covers users
until then. Reputation also builds automatically as more people run the app.

## Versioning

Drifterr ships **one product version**. Three places must always agree:

1. `apps/desktop/src-tauri/tauri.conf.json` → `version` (the app + updater compare this)
2. `apps/desktop/src-tauri/Cargo.toml` → `version` (and its `Cargo.lock` entry)
3. `[workspace.package] version` in the root `Cargo.toml` (all internal crates
   inherit it via `version.workspace = true`)

…and the **git tag** you push (`vX.Y.Z`) must match them. The desktop app sets
`DRIFTERR_APP_VERSION` so the embedded proxy reports the app version at
`GET /config`; the landing page reads the latest GitHub release tag. So a single
`X.Y.Z` flows everywhere: crates → binary → `/config` → settings panel → landing
badge → git tag.

**SemVer.** MAJOR for breaking changes to the detection contract or the control
API, MINOR for features, PATCH for fixes. Pre-1.0 we allow MINOR to carry small
breaking changes.

**Bumping (do all in one commit):**

```bash
NEW=0.2.4
sed -i "s/^version = \".*\"/version = \"$NEW\"/" Cargo.toml apps/desktop/src-tauri/Cargo.toml
sed -i "s/\"version\": \".*\"/\"version\": \"$NEW\"/" apps/desktop/src-tauri/tauri.conf.json
cargo update -w                                   # refresh workspace Cargo.lock
(cd apps/desktop/src-tauri && cargo update)       # refresh the app's Cargo.lock
# update release.yml's workflow_dispatch default to vNEW, then commit + tag vNEW
```

Verify concordance before tagging: the three `version` fields, both `Cargo.lock`
files, and the tag all read the same `X.Y.Z`.

## Cut a release

```bash
# from an up-to-date main
git tag v0.1.0
git push origin v0.1.0
```

This runs the release workflow across macOS (Apple silicon + Intel), Linux and
Windows on the **lexical** build path (default).

> **Semantic (ONNX) is OFF by default and currently Windows-broken.** The ONNX
> runtime fails to link on the Windows runner (`LNK2038`: `ort_sys` builds the
> CRT as `/MD` while `esaxx-rs` builds it `/MT`). Until that's resolved, leave
> the **Bundle the ONNX semantic model** box unchecked. macOS/Linux semantic
> builds work, but a release needs all three platforms, so ship lexical.

It creates a **draft** release with:

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
