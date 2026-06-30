// Headless checks for the landing page: it renders, and the download button
// adapts to the visitor's OS. Run: npm test (from apps/landing).

import { chromium } from "playwright";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, join, extname } from "node:path";

const DIR = join(dirname(fileURLToPath(import.meta.url)), "..");
const MIME = {
  ".html": "text/html",
  ".css": "text/css",
  ".js": "text/javascript",
  ".png": "image/png",
};

function serve() {
  return new Promise((resolve) => {
    const s = createServer(async (req, res) => {
      let p = req.url.split("?")[0];
      if (p === "/") p = "/index.html";
      try {
        const body = await readFile(join(DIR, p));
        res.writeHead(200, { "content-type": MIME[extname(p)] || "text/plain" });
        res.end(body);
      } catch {
        res.writeHead(404);
        res.end("nf");
      }
    });
    s.listen(0, "127.0.0.1", () => resolve(s));
  });
}

let failures = 0;
const check = (c, m) => (c ? console.log("  ✓ " + m) : (failures++, console.error("  ✗ " + m)));

const MAC_UA = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Safari/605.1";
const WIN_UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/120 Safari/537.36";

async function main() {
  const server = await serve();
  const { port } = server.address();
  const url = `http://127.0.0.1:${port}/index.html`;
  const browser = await chromium.launch(
    process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}
  );

  // Keep the test hermetic: neutralize the (production) accounts config so the
  // lazily-loaded Supabase client never touches the network during the run.
  // config.js uses `??=`, so pre-setting these empty wins.
  const neutralizeAccounts = (page) =>
    page.addInitScript(() => {
      window.DRIFTERR_SUPABASE_URL = "";
      window.DRIFTERR_SUPABASE_ANON_KEY = "";
    });

  for (const [name, ua] of [["macOS", MAC_UA], ["Windows", WIN_UA]]) {
    const ctx = await browser.newContext({ userAgent: ua });
    const page = await ctx.newPage();
    await neutralizeAccounts(page);
    await page.goto(url);
    console.log(name + " visitor:");
    const label = await page.locator("#download").textContent();
    check(label.includes("Download"), "download button says Download");
    const href = await page.locator("#download").getAttribute("href");
    check(href.endsWith("/download"), "download links to the /download page (own domain)");
    check(!/github\.com/.test(href), "download does not point at github.com");
    check((await page.locator("h1").textContent()).length > 10, "hero headline renders");
    check((await page.locator(".card").count()) === 6, "six feature cards");
    await ctx.close();
  }

  // --- download hub page ---
  {
    const page = await (await browser.newContext({ userAgent: MAC_UA })).newPage();
    await neutralizeAccounts(page);
    await page.goto(`http://127.0.0.1:${port}/download.html`);
    await page.waitForFunction(() => document.getElementById("rec-label").textContent.includes("Download"));
    console.log("download page:");
    check((await page.locator("#rec-label").textContent()).includes("macOS"), "recommended card detects macOS");
    check((await page.locator(".dl[data-os]").count()) === 4, "four platform tiles");
    check((await page.locator(".copy[data-cmd]").count()) === 3, "three CLI commands");
    const html = await page.content();
    check(!/github\.com/.test(html), "download page exposes no github.com link");
  }

  // --- legal pages ---
  {
    const page = await (await browser.newContext()).newPage();
    await neutralizeAccounts(page);
    console.log("legal pages:");
    for (const [path, heading] of [["/privacy.html", "Privacy Policy"], ["/terms.html", "Terms of Service"]]) {
      await page.goto(`http://127.0.0.1:${port}${path}`);
      check((await page.locator("h1").textContent()) === heading, `${path} renders "${heading}"`);
      check((await page.locator('footer a[href="/privacy"]').count()) === 1, `${path} footer links Privacy`);
      check((await page.locator('footer a[href="/terms"]').count()) === 1, `${path} footer links Terms`);
    }
  }

  await browser.close();
  server.close();
  if (failures) {
    console.error(`\n${failures} check(s) failed`);
    process.exit(1);
  }
  console.log("\nAll landing checks passed.");
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
