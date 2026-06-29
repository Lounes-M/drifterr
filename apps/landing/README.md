# Drifterr landing page

A static, zero-build marketing page (modern SaaS look, liquid-glass accents,
Satoshi via Fontshare CDN). The **Download** button detects the visitor's OS and
links to the GitHub Releases page (where the installers built by
`.github/workflows/release.yml` land).

```
index.html     hero · how-it-works · features · download · footer
styles.css     light theme, Ocean Blue accents, reveal-on-scroll
app.js         OS detection → adaptive download label/link
assets/        product screenshots
tests/         headless checks (Playwright)
```

## Preview

```bash
cd apps/landing
python3 -m http.server 8080   # then open http://localhost:8080
```

## Test

```bash
npm install && npx playwright install && npm test
```

## Deploy

`.github/workflows/pages.yml` publishes this folder to **GitHub Pages** on push
to `main`.

**Custom domain:** once you've bought one, add a file `apps/landing/CNAME`
containing just the domain (e.g. `drifterr.app`), set it in repo
**Settings → Pages → Custom domain**, and point the domain's DNS at GitHub Pages.
The site is fully static, so it also drops onto Vercel/Netlify/any host as-is.
