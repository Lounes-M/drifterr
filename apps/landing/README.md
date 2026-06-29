# Drifterr landing page

A static, zero-build marketing page (modern SaaS look, liquid-glass accents,
Satoshi via Fontshare CDN). The **Download** button detects the visitor's OS and
links to `/download/<os>` **on our own domain** — a serverless redirect resolves
the latest installer (built by `.github/workflows/release.yml`) and streams it
straight to the visitor, so nobody is sent to the GitHub repo.

```
index.html       hero · how-it-works · features · download · footer
styles.css       light theme, Ocean Blue accents, reveal-on-scroll
app.js           OS detection → adaptive download label, /download/<os> link
api/download.js  serverless redirect: latest release asset for the OS → 302
vercel.json      pretty-URL rewrites (/download/<os> → /api/download?os=<os>)
assets/          product screenshots
tests/           headless checks (Playwright)
```

## How the download stays on our domain

1. The button points at `/download/mac` (or `/win`, `/linux`).
2. `vercel.json` rewrites that to the `api/download` serverless function.
3. The function asks the GitHub API for the **latest release**, picks the asset
   matching the OS (universal → Apple silicon → Intel for macOS; NSIS `.exe` →
   `.msi` for Windows; `.AppImage` → `.deb` for Linux) and `302`s straight to the
   binary. The browser downloads the file; the repo page is never shown.
4. Before the first release exists (or on an API hiccup) it serves a friendly
   "coming soon" page instead of bouncing to GitHub.

`api/download.js` works unauthenticated; set a `GITHUB_TOKEN` env var in Vercel
only if you hit the API rate limit.

## Preview

```bash
cd apps/landing
python3 -m http.server 8080   # static preview (the /download redirect needs Vercel)
# or, to exercise the serverless function locally:
npx vercel dev
```

## Test

```bash
npm install && npx playwright install && npm test
```

## Deploy — Vercel

1. **Import** the repo in Vercel and set the **Root Directory** to `apps/landing`.
   `vercel.json` handles the rest (no build step, framework = "Other").
2. **Custom domain:** add it under **Project → Settings → Domains** and point the
   domain's DNS at Vercel (Vercel shows the exact records). SSL is automatic.

The site is otherwise plain static files, so it also drops onto Netlify or any
host that supports serverless functions / redirects.
