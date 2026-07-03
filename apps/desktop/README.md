# Drifterr desktop (menubar)

The menubar app: a tray icon whose color tracks your session state, and a
dropdown panel that **names the triggering signal**.

## Layout

```
apps/desktop/
├── ui/                 # the panel — plain HTML/CSS/JS, no build step
│   ├── index.html
│   ├── styles.css
│   ├── app.js          # polls the control API, renders the panel
│   └── tests/          # headless Playwright verification of the rendering
├── src-tauri/          # the native tray shell (Tauri 2)
│   ├── src/lib.rs      # tray + state-colored icon + click-to-toggle window
│   ├── tauri.conf.json
│   └── icons/          # generated tray + app icons
└── scripts/gen_icons.py
```

The panel and the tray both read the proxy's **control API** (`GET /status`),
the same contract the Rust e2e tests assert. The UI assets are shared: the proxy
serves them at `http://<control-addr>/` for instant browser use, and the Tauri
webview loads the same files.

## Look & fonts

The panel is a real **"liquid glass"** material (iOS-26-like): strongly
translucent frosted glass that floats over an ambient backdrop and refracts it
(in the Tauri menubar that's the desktop wallpaper; the browser dashboard shows
a soft drifting ambient that stands in for it). A specular highlight sweeps the
surface; cards and buttons are frosted with bright rim light. **Blue is reserved
for buttons/accents only.** Smooth and animated (drifting backdrop, sheen sweep,
spring reveals, button shine) — all disabled under `prefers-reduced-motion`.

For full desktop vibrancy in the packaged app, the Tauri window is transparent;
the ambient backdrop is the dashboard/preview stand-in.

Typeface is **Satoshi**. Drop the font files into
[`ui/public/fonts/`](ui/public/fonts) (see the README there for the exact
filenames) — the CSS `@font-face` is already wired and the proxy serves
`/public/fonts/*`, so they load in both the browser dashboard and the Tauri
webview. Until then it falls back to the system sans-serif.

## Run it

**As a browser panel (no Tauri needed)** — start the proxy and open the
dashboard it serves:

```bash
cargo run -p drifterr-proxy        # proxy :8787, control/dashboard :8788
open http://127.0.0.1:8788/        # the panel, live
```

**As a native menubar app** (on a machine with the Tauri prerequisites —
webkit2gtk on Linux, Xcode CLT on macOS). The app **embeds the proxy** and starts
it in-process, so this is the only thing you run:

```bash
cargo install tauri-cli --version '^2'
cd apps/desktop/src-tauri
cargo tauri dev      # menubar + proxy (:8787) + control API (:8788), all in one
```

To build installers (`.dmg` / `.AppImage` / `.deb` / `.msi`) and set up signing
and auto-update, see [`PACKAGING.md`](PACKAGING.md).

> The Tauri shell is intentionally excluded from the Cargo workspace and is not
> compiled in the headless CI used for the rest of the repo (it needs platform
> GUI libraries). The panel UI, by contrast, is fully tested headlessly.

## Test the UI

```bash
cd apps/desktop/ui
npm install
npm test            # drives the rendered panel in headless Chromium
```

The test stubs `/status` with red / green / offline payloads and asserts the DOM
renders correctly (state color, named triggering signal, offending span,
per-signal list, offline banner). In a sandbox that pins a specific browser,
set `CHROMIUM_PATH` to its executable.

## Semantic model (ONNX — optional, off by default)

The default embedder is local + lexical (zero-network, zero-cost). For **true
semantic** drift detection ("car" ↔ "automobile"), the app can bundle a small
ONNX sentence-transformer. It's opt-in so the default release build pulls no
native runtime and ships nothing extra.

Two commands — the committed default config is never touched (the model is
injected as a resource only for this build via `--config`), so the normal
release stays untouched:

```bash
# 1. Fetch the model → src-tauri/models/embed/{model.onnx, tokenizer.json}
apps/desktop/scripts/fetch-model.sh            # bge-small-en-v1.5 (gitignored)

# 2. Build with the feature + bundle the model as a resource for THIS build:
cd apps/desktop/src-tauri
cargo tauri build --features semantic \
  --config '{"bundle":{"resources":["models/embed/*"]}}'
```

At launch the app auto-detects `models/embed/model.onnx` in its resource dir and
points the embedder at it (`start_embedded_proxy`); if it's absent, or the app
was built without `--features semantic`, it silently uses the lexical embedder —
detection never breaks.

**Why bother:** measured, not assumed — the semantic model takes goal-drift
detection from *missed* (lexical) to *caught* on the eval set (66.7% → 100%).
See [`crates/embeddings/README.md`](../../crates/embeddings/README.md) for the
numbers. Trade-off: ~130 MB added to the bundle, and the native runtime must be
covered by macOS notarization (it's inside the `.app`, so it is).

## Regenerate icons

```bash
python3 apps/desktop/scripts/gen_icons.py
```
