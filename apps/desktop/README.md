# Drifterr desktop (menubar)

The menubar app: a tray icon whose color tracks your session state, and a
dropdown panel that **names the triggering signal**.

![Drifting state](../../docs/menubar-red.png)
![Aligned state](../../docs/menubar-green.png)

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

The panel is **glassmorphism** (translucent + backdrop blur, layered) on the
**Ocean Blue Serenity** palette, with subtle entrance/reveal/pulse animations
(disabled under `prefers-reduced-motion`).

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
webkit2gtk on Linux, Xcode CLT on macOS):

```bash
cargo install tauri-cli --version '^2'
cd apps/desktop/src-tauri
cargo tauri dev
```

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

## Regenerate icons

```bash
python3 apps/desktop/scripts/gen_icons.py
```
