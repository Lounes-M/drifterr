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

## One-time setup — entitlement signing key

Release builds verify the plan the panel reports instead of trusting it (see
`docs/ACCOUNTS.md`). That needs an Ed25519 pair: the private half signs assertions
in the Supabase `/me` function, the public half is compiled into the app.

Generate one:

```bash
deno eval 'const k = await crypto.subtle.generateKey({name:"Ed25519"}, true, ["sign","verify"]);
  const pk = new Uint8Array(await crypto.subtle.exportKey("raw", k.publicKey));
  const sk = new Uint8Array((await crypto.subtle.exportKey("pkcs8", k.privateKey)).slice(-32));
  const b = (u) => btoa(String.fromCharCode(...u)).replace(/\+/g,"-").replace(/\//g,"_").replace(/=+$/,"");
  console.log("public :", b(pk)); console.log("private:", b(sk));'
```

Then:

1. **Supabase** → function secret `ENTITLEMENT_SIGNING_KEY` = the *private* value.
2. **GitHub** → repo secret `DRIFTERR_ENTITLEMENT_PUBKEY` = the *public* value.

> The public key is read with `option_env!`, so it is baked in at **compile** time.
> A release built without the secret accepts whatever plan the panel asserts and
> reports `"verified": false` — a supported state for a fork or a self-hoster, and
> the wrong one for a shipped build. The workflow prints a loud
> `Unverified entitlements` warning whenever the secret is missing, exactly like
> the unsigned-build warning.

**Rotation is additive.** Ship a build that carries the new public key *before*
switching the Supabase secret, or every signed-in customer is downgraded to Free
for the window in between.


## Signing and notarization (removes the install warning)

**This is the highest-impact item in the whole install funnel.** Unsigned builds
make macOS say *"Drifterr is damaged and can't be opened"* and make Windows hide
the install button behind SmartScreen. Most people who hit that do not work around
it — they conclude the download is broken and leave, and we never hear about it.

The workflow is fully wired for both platforms and **signs automatically as soon
as the secrets exist**, with no workflow edit needed:

- **No certificates configured** → the build still succeeds and ships unsigned,
  and the job log carries a loud `Unsigned build` warning. A missing secret can
  never fail a release.
- **Certificates configured** → artifacts are signed (and notarized on macOS),
  then *verified* in a follow-up step, so a silently-unsigned release is not
  possible.

While the `Unsigned build` warning still appears, the first-launch instructions on
[drifterr.app/download](https://drifterr.app/download) (`apps/landing/download.html`,
section `#firstrun`) **must stay up** — that block is the only thing standing
between a user and a dead end. Remove it in the same PR that lands working
signatures, not before.

> Implementation note: `APPLE_CERTIFICATE` is deliberately *not* passed to
> tauri-action. It treats the variable as "signing requested" whenever it is
> merely defined, so an empty value made it run `security import` on nothing and
> fail the macOS build outright — which is why signing sat disabled. The workflow
> now imports the certificate into a throwaway runner keychain itself and passes
> only the identity.

### macOS — signed + notarized (Gatekeeper is strict)

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
4. Cut a release. The universal `.app`/`.dmg` is signed and notarized, then
   verified with `codesign --verify --deep --strict` and `spctl --assess` — the same
   check a user's Mac performs. (First notarization can add a few minutes.)

`APPLE_CERTIFICATE` + `APPLE_SIGNING_IDENTITY` alone enable signing; adding
`APPLE_ID` / `APPLE_PASSWORD` / `APPLE_TEAM_ID` also enables notarization. Signing
without notarizing still shows a warning, so the job emits a warning telling you
so — treat all six as one unit.

### Windows — Authenticode (removes SmartScreen)

Get a code-signing certificate: an OV/EV cert from a CA, or **Azure Trusted
Signing** (cheapest route for an indie). Export it as a `.pfx`, then base64 it
(`base64 -w0 drifterr.pfx`) and add:

| Secret | Value |
|---|---|
| `WINDOWS_CERTIFICATE` | the base64 `.pfx` |
| `WINDOWS_CERTIFICATE_PASSWORD` | its password (omit if none) |

Signing happens **during bundling**, via `apps/desktop/src-tauri/windows-sign.conf.json`
(added to the build args only when the certificate exists) which points
`bundle.windows.signCommand` at `apps/desktop/scripts/sign-windows.ps1`. That way
both the app `.exe` and the installer are signed, each one SHA-256 signed,
RFC-3161 timestamped, and verified with `signtool verify /pa` before the build
continues.

Timestamping is not optional: without it every signature stops validating the day
the certificate expires, retroactively breaking releases already in the wild.
Override the default timestamp authority with `WINDOWS_TIMESTAMP_URL` if needed.

Note that SmartScreen reputation also accrues over time — a brand-new OV
certificate may still warn for the first while, though far less than no signature
at all. EV certificates get reputation immediately.

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
