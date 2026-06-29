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

  for (const [name, ua, want] of [
    ["macOS", MAC_UA, "macOS"],
    ["Windows", WIN_UA, "Windows"],
  ]) {
    const ctx = await browser.newContext({ userAgent: ua });
    const page = await ctx.newPage();
    await page.goto(url);
    await page.waitForFunction(() => document.getElementById("download").textContent.includes("Download"));
    console.log(name + " visitor:");
    const label = await page.locator("#download").textContent();
    check(label.includes(want), `download button says "${want}"`);
    const href = await page.locator("#download").getAttribute("href");
    check(href.includes("/releases"), "download links to releases");
    check((await page.locator("h1").textContent()).length > 10, "hero headline renders");
    check((await page.locator(".card").count()) === 6, "six feature cards");
    await ctx.close();
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
