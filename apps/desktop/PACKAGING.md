# Packaging Drifterr (desktop)

Drifterr ships as **one app**: the menubar shell embeds the local proxy and starts
it in-process (see `src-tauri/src/lib.rs` → `start_embedded_proxy`). Installing the
app gives you the proxy (`:8787`) + control API/dashboard (`:8788`) + the menubar —
nothing else to run.

> ⚠️ The Tauri app is **excluded from the repo's normal CI** (it needs platform GUI
> libraries) and was **not built in the authoring sandbox**. The steps below are the
> intended, conventional Tauri 2 flow — verify them on a real machine for the first
> release.

## Prerequisites

- Rust (stable) + the Tauri CLI: `cargo install tauri-cli --version '^2'`
- Platform deps:
  - **macOS**: Xcode Command Line Tools
  - **Linux**: `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf`
  - **Windows**: WebView2 (preinstalled on Win11) + MSVC build tools

## Icons (one-time)

Bundling needs platform icon formats (`.icns`, `.ico`) in addition to the PNGs.
Generate the full set from a 1024×1024 source:

```bash
cd apps/desktop/src-tauri
cargo tauri icon ../../../path/to/icon-1024.png
```

## Dev run

```bash
cd apps/desktop/src-tauri
cargo tauri dev
```

## Build installers

```bash
cd apps/desktop/src-tauri
cargo tauri build
```

Outputs land under `src-tauri/target/release/bundle/`:
`.dmg`/`.app` (macOS), `.AppImage`/`.deb` (Linux), `.msi`/NSIS `.exe` (Windows).

CI does this across all three OSes on a version tag — see
[`.github/workflows/release.yml`](../../.github/workflows/release.yml):

```bash
git tag v0.1.0 && git push origin v0.1.0   # → draft GitHub release with installers
```

## Code signing & notarization (optional, recommended)

Unsigned apps trigger OS warnings. Add these as **GitHub repo secrets** and
uncomment the matching `env:` lines in `release.yml`:

- **macOS**: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`,
  `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`.
- **Windows**: a code-signing cert (Authenticode) per Tauri's Windows signing docs.

## Auto-update (optional)

1. Generate a signing keypair:
   ```bash
   cargo tauri signer generate -w ~/.drifterr/updater.key
   ```
2. Put the **public** key in `tauri.conf.json` → `plugins.updater.pubkey`
   (replace `REPLACE_WITH_OUTPUT_OF_tauri_signer_generate`).
3. Set `bundle.createUpdaterArtifacts: true` in `tauri.conf.json`.
4. Add the **private** key as secrets and uncomment `TAURI_SIGNING_PRIVATE_KEY`
   (+ password) in `release.yml`.

The app polls the `endpoints` in `plugins.updater` (default: this repo's latest
release `latest.json`) and self-updates. The plugin is already registered in
`run()`; until a pubkey is set, update checks are simply inert.
