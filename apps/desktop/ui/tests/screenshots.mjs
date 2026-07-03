// Design-audit harness: render the panel at the exact Tauri window size
// (380×600) in every state and screenshot it, so alignment/centering can be
// eyeballed and fixed. Not part of `npm test` — run explicitly:
//
//   CHROMIUM_PATH=/path/to/chrome node tests/screenshots.mjs [outDir]
//
import { chromium } from "playwright";
import { createServer } from "node:http";
import { readFile, mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, join, extname } from "node:path";

const UI_DIR = join(dirname(fileURLToPath(import.meta.url)), "..");
const OUT = process.argv[2] || "/tmp/drifterr-shots";
const MIME = { ".html": "text/html", ".css": "text/css", ".js": "text/javascript" };

const SIG = (signal, state, detail, extra = {}) => ({ signal, state, detail, ...extra });

const RED = {
  current: {
    sessionId: "sess-abc", model: "claude-opus-4-x", state: "red",
    saturationPct: 68, driftScore: 82, exact: true,
    history: [10, 20, 15, 40, 55, 30, 70, 82],
    triggering: SIG("constraint", "red", 'constraint c1 violated: "TypeScript only, no JS files"', { constraintId: "c1", span: "auth.js" }),
    signals: [
      SIG("constraint", "red", "violated c1 — created auth.js", { constraintId: "c1", span: "auth.js" }),
      SIG("saturation", "amber", "context 68% full (exact)"),
      SIG("goal_alignment", "green", "on track with the stated goal"),
    ],
  },
  sessions: [], entitlement: { driftMap: true },
};
const GREEN = {
  current: {
    sessionId: "sess-abc", model: "gpt-4o", state: "green",
    saturationPct: 24, driftScore: 8, exact: true,
    history: [5, 8, 6, 10, 8], triggering: null,
    signals: [
      SIG("constraint", "green", "no violations"),
      SIG("saturation", "green", "context 24% full (exact)"),
    ],
  },
  sessions: [], entitlement: { driftMap: true },
};
const INTENT = {
  goal: "Ship the billing API in strict TypeScript",
  constraints: [
    { id: "c1", text: "TypeScript only, no JS files", kind: "tech", checkable: "deterministic", active: true },
    { id: "c2", text: "Keep the tone formal and concise", kind: "tone", checkable: "judge", active: true },
  ],
  pending: false,
};
const HISTORY = [
  { sessionId: "s3", model: "claude-opus-4-x", goal: "Ship the billing API in strict TypeScript", state: "red", turns: 14, lastActivity: Date.now() - 5400000 },
  { sessionId: "s2", model: "gpt-4o", goal: "Draft the launch blog post", state: "amber", turns: 6, lastActivity: Date.now() - 86400000 },
  { sessionId: "s1", model: "gpt-4o", goal: "", state: "green", turns: 3, lastActivity: Date.now() - 3 * 86400000 },
];
const CONFIG = { version: "0.1.5", provider: "OpenRouter", openaiUpstream: "https://openrouter.ai/api", anthropicUpstream: "https://api.anthropic.com", persisted: true, judge: "openai/gpt-4o-mini", autoReanchor: false };
const PROVIDERS = { current: "openrouter", providers: [{ id: "openrouter", label: "OpenRouter" }, { id: "openai", label: "OpenAI" }, { id: "gemini", label: "Google Gemini" }, { id: "anthropic", label: "Anthropic" }] };
const REANCHOR = {
  snapshot: "# Re-anchor\n\n## Goal\nShip the billing API in strict TypeScript\n\n## Active constraints\n- [tech] TypeScript only, no JS files\n- [tone] Keep the tone formal\n\n## Where we are\n- Status: Drifting — created auth.js\n- Context: 68% full (exact)\n",
  preamble: "[Drifterr re-anchor] Binding constraints for this task:\n- TypeScript only, no JS files\n- Keep the tone formal\nPlease honor these from now on.",
};

function serveUi() {
  return new Promise((resolve) => {
    const server = createServer(async (req, res) => {
      const p = req.url.split("?")[0];
      const file = p === "/" ? "index.html" : p.slice(1);
      try {
        const buf = await readFile(join(UI_DIR, file));
        res.writeHead(200, { "content-type": MIME[extname(file)] || "text/plain" });
        res.end(buf);
      } catch { res.writeHead(404); res.end("nf"); }
    });
    server.listen(0, "127.0.0.1", () => resolve(server));
  });
}

async function routeAll(page, status = GREEN) {
  await page.addInitScript(() => {
    window.DRIFTERR_SUPABASE_URL = ""; window.DRIFTERR_SUPABASE_ANON_KEY = "";
    try { localStorage.setItem("drifterr_onboarded", "1"); } catch (_e) {}
  });
  const j = (body) => (route) => route.fulfill({ contentType: "application/json", body: JSON.stringify(body) });
  await page.route("**/status*", j(status));
  await page.route("**/intent*", j(INTENT));
  await page.route("**/config*", j(CONFIG));
  await page.route("**/providers*", j(PROVIDERS));
  await page.route("**/auto-reanchor*", j({ on: false, allowed: true, effective: false }));
  await page.route("**/judge*", j({ enabled: true, label: "openai/gpt-4o-mini" }));
  await page.route("**/history*", j(HISTORY));
  await page.route("**/reanchor*", j(REANCHOR));
}

async function shoot(browser, name, status, prep) {
  const ctx = await browser.newContext({ viewport: { width: 380, height: 600 }, deviceScaleFactor: 2, reducedMotion: 'reduce' });
  const page = await ctx.newPage();
  await routeAll(page, status);
  await page.goto(url);
  await page.waitForTimeout(700); // let splash finish + first poll render
  if (prep) await prep(page);
  await page.waitForTimeout(350);
  await page.screenshot({ path: join(OUT, name + ".png"), fullPage: true });
  await ctx.close();
  console.log("  ✓ " + name);
}

let url;
async function main() {
  await mkdir(OUT, { recursive: true });
  const server = await serveUi();
  url = `http://127.0.0.1:${server.address().port}/`;
  const browser = await chromium.launch(
    process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}
  );
  await shoot(browser, "01-green", GREEN);
  await shoot(browser, "02-red", RED);
  await shoot(browser, "03-red-reanchor", RED, async (p) => { await p.locator("#reanchor-btn").click(); await p.waitForFunction(() => !document.getElementById("reanchor").hidden); });
  await shoot(browser, "04-intent-editor", GREEN, async (p) => { await p.locator("#intent-edit").click(); });
  await shoot(browser, "05-settings", GREEN, async (p) => { await p.locator("#gear").click(); await p.waitForTimeout(300); });
  await shoot(browser, "06-history", GREEN, async (p) => { await p.locator("#history-btn").click(); await p.waitForTimeout(300); });
  await shoot(browser, "07-empty", { current: null, sessions: [], entitlement: {} });
  const SHIFT = JSON.parse(JSON.stringify(GREEN));
  SHIFT.current.intentShift = { from: "Refactor the auth module to argon2", to: "Build a React analytics dashboard" };
  await shoot(browser, "10-intent-shift", SHIFT);
  // Onboarding: clear the flag first.
  {
    const ctx = await browser.newContext({ viewport: { width: 380, height: 600 }, deviceScaleFactor: 2, reducedMotion: 'reduce' });
    const page = await ctx.newPage();
    await page.addInitScript(() => { window.DRIFTERR_SUPABASE_URL = ""; window.DRIFTERR_SUPABASE_ANON_KEY = ""; });
    const j = (body) => (route) => route.fulfill({ contentType: "application/json", body: JSON.stringify(body) });
    await page.route("**/status*", j({ current: null, sessions: [] }));
    await page.route("**/providers*", j(PROVIDERS));
    await page.route("**/intent*", j(INTENT));
    await page.goto(url);
    await page.waitForFunction(() => !document.getElementById("onboarding").hidden);
    await page.waitForTimeout(400);
    await page.screenshot({ path: join(OUT, "08-onboarding-1.png"), fullPage: true });
    for (let i = 0; i < 3; i++) { await page.locator("#onb-next").click(); await page.waitForTimeout(250); }
    await page.screenshot({ path: join(OUT, "09-onboarding-intent.png"), fullPage: true });
    await ctx.close();
    console.log("  ✓ 08/09-onboarding");
  }
  await browser.close();
  server.close();
  console.log("\nShots in " + OUT);
}
main().catch((e) => { console.error(e); process.exit(1); });
