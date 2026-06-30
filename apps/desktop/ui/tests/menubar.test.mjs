// Headless UI verification for the Drifterr menubar panel.
//
// Serves the static UI, then drives the real rendered DOM with Playwright while
// stubbing the control API's `GET /status`. This verifies the rendering logic
// the user actually sees; the proxy → /status JSON path is covered separately by
// the Rust e2e tests. Together they cover the whole chain.
//
// Run: npm test   (from apps/desktop/ui)

import { chromium } from "playwright";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, join, extname } from "node:path";

const UI_DIR = join(dirname(fileURLToPath(import.meta.url)), "..");
const MIME = { ".html": "text/html", ".css": "text/css", ".js": "text/javascript" };

// --- canned status payloads ------------------------------------------------

const RED = {
  current: {
    sessionId: "sess-abc",
    model: "claude-opus-4-x",
    state: "red",
    saturationPct: 42,
    exact: true,
    triggering: {
      signal: "constraint",
      state: "red",
      detail: 'constraint c1 violated: "TypeScript only, no JS"',
      constraintId: "c1",
      span: ".js",
    },
    signals: [
      { signal: "constraint", state: "red", detail: "violated c1", constraintId: "c1", span: ".js" },
      { signal: "saturation", state: "green", detail: "context 42% full (exact)" },
    ],
  },
  sessions: [],
};

const GREEN = {
  current: {
    sessionId: "sess-abc",
    model: "claude-opus-4-x",
    state: "green",
    saturationPct: 20,
    exact: true,
    triggering: null,
    signals: [{ signal: "saturation", state: "green", detail: "context 20% full (exact)" }],
  },
  sessions: [],
};

// --- tiny static file server ----------------------------------------------

function serveUi() {
  return new Promise((resolve) => {
    const server = createServer(async (req, res) => {
      let path = req.url.split("?")[0];
      if (path === "/") path = "/index.html";
      try {
        const body = await readFile(join(UI_DIR, path));
        res.writeHead(200, { "content-type": MIME[extname(path)] || "text/plain" });
        res.end(body);
      } catch {
        res.writeHead(404);
        res.end("not found");
      }
    });
    server.listen(0, "127.0.0.1", () => resolve(server));
  });
}

// --- assertions ------------------------------------------------------------

let failures = 0;
function check(cond, msg) {
  if (cond) {
    console.log("  ✓ " + msg);
  } else {
    failures++;
    console.error("  ✗ " + msg);
  }
}

async function main() {
  const server = await serveUi();
  const { port } = server.address();
  const url = `http://127.0.0.1:${port}/index.html`;

  // Use a pre-installed Chromium when the env pins one (CI / sandboxes whose
  // browser build differs from the npm package's expected build). Falls back to
  // Playwright's managed browser locally.
  const launchOpts = process.env.CHROMIUM_PATH
    ? { executablePath: process.env.CHROMIUM_PATH }
    : {};
  const browser = await chromium.launch(launchOpts);
  const context = await browser.newContext({ permissions: ["clipboard-read", "clipboard-write"] });
  const page = await context.newPage();

  // Keep the test hermetic: neutralize the (production) accounts config so the
  // login gate stays inactive and nothing reaches the network. config.js uses
  // `??=`, so pre-setting these empty wins. This verifies the accounts-free
  // panel behaviour; the gated path is covered by its own rendering check.
  await page.addInitScript(() => {
    window.DRIFTERR_SUPABASE_URL = "";
    window.DRIFTERR_SUPABASE_ANON_KEY = "";
  });

  // The route handler returns whatever `scenario` currently points at, so we
  // can flip server state between polls.
  let scenario = RED;
  await page.route("**/status*", (route) =>
    route.fulfill({ contentType: "application/json", body: JSON.stringify(scenario) })
  );

  await page.goto(url);

  console.log("RED scenario:");
  await page.waitForFunction(() => document.getElementById("state-label").textContent === "Drifting");
  check(
    (await page.locator("#dot").getAttribute("class")).includes("red"),
    "status dot is red"
  );
  check(await page.locator("#trigger").isVisible(), "trigger block is shown");
  check((await page.locator("#trigger-signal").textContent()) === "Constraints", "trigger names the Constraints signal");
  check((await page.locator("#trigger-detail").textContent()).includes("c1"), "trigger detail mentions c1");
  check((await page.locator("#trigger-span").textContent()) === ".js", "offending span shown as a chip");
  check((await page.locator("#sat-meta").textContent()).includes("exact"), "saturation marked exact");
  check((await page.locator("#signals .signal-row").count()) === 2, "both signals listed");

  console.log("RE-ANCHOR flow:");
  await page.route("**/reanchor*", (route) =>
    route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        snapshot: "# Re-anchor\n\n## Goal\nRefactor auth in strict TypeScript\n\n## Active constraints\n- [tech] TypeScript only, no JS files\n",
        preamble: "[Drifterr re-anchor] Binding constraints...",
      }),
    })
  );
  check(!(await page.locator("#reanchor").isVisible()), "re-anchor hidden by default");
  await page.locator("#reanchor-btn").click();
  await page.waitForFunction(() => !document.getElementById("reanchor").hidden);
  check(await page.locator("#reanchor").isVisible(), "re-anchor button reveals the snapshot");
  check(
    (await page.locator("#reanchor-text").textContent()).includes("TypeScript only, no JS files"),
    "snapshot shows the constraint"
  );
  await page.locator("#reanchor-copy").click();
  await page.waitForFunction(() => document.getElementById("reanchor-copy").textContent === "Copied!");
  check(true, "copy button copies to clipboard");
  const clip = await page.evaluate(() => navigator.clipboard.readText());
  check(clip.includes("# Re-anchor"), "clipboard holds the snapshot");
  await page.locator("#reanchor-close").click();
  check(!(await page.locator("#reanchor").isVisible()), "close hides the snapshot");

  console.log("GREEN scenario (flip + wait for poll):");
  scenario = GREEN;
  await page.waitForFunction(() => document.getElementById("state-label").textContent === "Aligned", null, {
    timeout: 5000,
  });
  check((await page.locator("#dot").getAttribute("class")).includes("green"), "dot flips to green");
  check(!(await page.locator("#trigger").isVisible()), "trigger block hidden when aligned");

  console.log("GATING (drift map lock):");
  const baseCur = { sessionId: "s", model: "m", state: "green", saturationPct: 10, exact: true, driftScore: 5, history: [5, 8, 12], signals: [] };
  scenario = { current: baseCur, sessions: [], entitlement: { plan: "free", driftMap: false }, sessionsLocked: 1 };
  await page.waitForFunction(() => document.getElementById("drift-map-section").classList.contains("locked"), null, { timeout: 5000 });
  check(await page.locator("#map-lock").isVisible(), "Free locks the drift map with a Pro badge");
  scenario = { current: baseCur, sessions: [], entitlement: { plan: "pro", driftMap: true }, sessionsLocked: 0 };
  await page.waitForFunction(() => !document.getElementById("drift-map-section").classList.contains("locked"), null, { timeout: 5000 });
  check(!(await page.locator("#map-lock").isVisible()), "Pro unlocks the drift map");

  console.log("SETTINGS view:");
  await page.route("**/config*", (route) =>
    route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        version: "0.0.1",
        provider: "OpenRouter",
        openaiUpstream: "https://openrouter.ai/api",
        anthropicUpstream: "https://api.anthropic.com",
        persisted: false,
        judge: "openai/gpt-4o-mini",
      }),
    })
  );
  check(!(await page.locator("#settings").isVisible()), "settings hidden by default");
  await page.locator("#gear").click();
  await page.waitForFunction(() => !document.getElementById("settings").hidden);
  check(await page.locator("#settings").isVisible(), "gear opens settings");
  await page.waitForFunction(
    () => document.getElementById("cfg-upstream").textContent.includes("openrouter")
  );
  check(
    (await page.locator("#cfg-upstream").textContent()).includes("openrouter.ai"),
    "settings shows OpenRouter upstream"
  );
  check((await page.locator("#cfg-provider").textContent()) === "OpenRouter", "settings names the active provider");
  check((await page.locator("#cfg-storage").textContent()) === "In-memory", "settings shows storage mode");
  check((await page.locator("#cfg-judge").textContent()).includes("gpt-4o-mini"), "settings shows judge model");
  await page.locator("#gear").click();
  check(!(await page.locator("#settings").isVisible()), "gear closes settings again");

  console.log("OFFLINE scenario:");
  await page.unroute("**/status*");
  await page.route("**/status*", (route) => route.abort());
  await page.waitForFunction(() => !document.getElementById("error").hidden, null, { timeout: 5000 });
  check(await page.locator("#error").isVisible(), "shows offline error when API unreachable");

  await browser.close();
  server.close();

  if (failures > 0) {
    console.error(`\n${failures} check(s) failed`);
    process.exit(1);
  }
  console.log("\nAll UI checks passed.");
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
