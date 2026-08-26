// Static config guards for the desktop app — pure Node, no browser.
//
// These assert the *shipping configuration* is internally consistent, catching
// the class of bugs that shipped silently in early 0.1.x:
//   * the CSP blocked the fonts + the whole Supabase login flow;
//   * `apiBase()` returned `http://tauri.localhost` on Windows/Linux so the
//     panel never reached the control API;
//   * styles.css referenced CSS variables that were never defined.
// None of these are caught by the DOM tests, so they get their own fast lint.
//
// Run: npm test   (wired after the Playwright suite in package.json)

import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const UI_DIR = join(dirname(fileURLToPath(import.meta.url)), "..");
const TAURI_CONF = join(UI_DIR, "..", "src-tauri", "tauri.conf.json");

let failures = 0;
function check(name, cond, detail) {
  if (cond) {
    console.log(`  ok   ${name}`);
  } else {
    failures++;
    console.error(`  FAIL ${name}${detail ? " — " + detail : ""}`);
  }
}

// --- 1. CSP allows every origin the shipped UI actually talks to ------------

{
  const conf = JSON.parse(await readFile(TAURI_CONF, "utf8"));
  const csp = conf?.app?.security?.csp || "";
  // Each host the panel + accounts flow depend on at runtime. If any is missing
  // the feature silently dies inside the webview (as fonts + login once did).
  const required = [
    "http://127.0.0.1:8788", // control API
    "https://esm.sh", // Supabase SDK (dynamic import in auth.js)
    "supabase.co", // auth + edge functions (connect-src)
    "fonts.googleapis.com", // font stylesheet
    "fonts.gstatic.com", // font files
  ];
  for (const host of required) {
    check(`CSP allows ${host}`, csp.includes(host), `csp = ${csp || "(empty)"}`);
  }
  // The window must be a sane size (a 0/undefined dimension = invisible panel).
  const win = conf?.app?.windows?.[0] || {};
  check("window has width/height", win.width > 0 && win.height > 0, JSON.stringify(win));
}

// --- 2. Every CSS variable used in styles.css is defined --------------------

{
  const css = await readFile(join(UI_DIR, "styles.css"), "utf8");
  const defined = new Set([...css.matchAll(/(--[a-z0-9-]+)\s*:/gi)].map((m) => m[1]));
  const used = new Set([...css.matchAll(/var\(\s*(--[a-z0-9-]+)/gi)].map((m) => m[1]));
  const missing = [...used].filter((v) => !defined.has(v));
  check("no undefined CSS variables", missing.length === 0, `missing: ${missing.join(", ")}`);
}

// --- 3. apiBase() reaches the control port from the Tauri webview ----------

{
  // Import the real module; the autostart block is guarded so this is inert.
  const { apiBase } = await import("../app.js");
  const CONTROL = "http://127.0.0.1:8788";

  const withEnv = (win, location, fn) => {
    const pw = globalThis.window, pl = globalThis.location;
    globalThis.window = win;
    globalThis.location = location;
    try { return fn(); } finally { globalThis.window = pw; globalThis.location = pl; }
  };

  // Windows/Linux Tauri origin — the exact regression that broke connection.
  check(
    "apiBase → control port on http://tauri.localhost",
    withEnv({ __TAURI_INTERNALS__: {} }, { origin: "http://tauri.localhost", hostname: "tauri.localhost" }, apiBase) === CONTROL,
  );
  // macOS Tauri origin.
  check(
    "apiBase → control port on tauri://localhost",
    withEnv({ __TAURI_INTERNALS__: {} }, { origin: "tauri://localhost", hostname: "localhost" }, apiBase) === CONTROL,
  );
  // Served by the control server itself → same-origin is preserved.
  check(
    "apiBase → same-origin when served by control server",
    withEnv({}, { origin: "http://127.0.0.1:8788", hostname: "127.0.0.1" }, apiBase) === CONTROL,
  );
  // Explicit override always wins.
  check(
    "apiBase honours window.DRIFTERR_API override",
    withEnv({ DRIFTERR_API: "http://example:9" }, { origin: "http://tauri.localhost", hostname: "tauri.localhost" }, apiBase) === "http://example:9",
  );
}

// --- every control-API call carries the pairing token -----------------------
//
// The control API is authenticated because it serves conversation content and a
// wildcard CORS policy once made every website a reader of it. That guarantee is
// only as good as the client: one `fetch(apiBase() + "/x")` that forgets the
// header is a broken feature, and one that *works* because a route was left
// public is a hole. So rather than trusting a convention, scan the source.
{
  const app = await readFile(join(UI_DIR, "app.js"), "utf8");

  // Strip comments so a code sample inside a doc block can't trip the scan.
  const code = app
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/^\s*\/\/.*$/gm, "");

  // Requests look like `f(apiBase() + "...", <init>)` or the same with `fetch`.
  // Match the call by balancing parentheses rather than by regex: an init can
  // contain parentheses (a ternary, a nested call), and a regex that stops at the
  // first `)` reports a false violation on exactly the calls most worth checking.
  const callAt = (src, open) => {
    let depth = 0;
    let str = null;
    for (let i = open; i < src.length; i++) {
      const c = src[i];
      if (str) {
        if (c === "\\") i++;
        else if (c === str) str = null;
        continue;
      }
      if (c === '"' || c === "'" || c === "`") str = c;
      else if (c === "(") depth++;
      else if (c === ")") {
        depth--;
        if (depth === 0) return src.slice(open, i + 1);
      }
    }
    return src.slice(open);
  };

  const calls = [];
  const callStart = /(?:^|[^.\w])(f|fetch)\(\s*apiBase\(\)/g;
  for (let m; (m = callStart.exec(code)); ) {
    calls.push(callAt(code, code.indexOf("(", m.index + m[0].indexOf(m[1]))));
  }
  check("panel makes control-API calls at all", calls.length > 10);
  const bare = calls.filter((c) => !c.includes("withAuth("));
  check(
    `every control-API call uses withAuth (${calls.length} calls, ${bare.length} bare)`,
    bare.length === 0,
  );
  for (const c of bare.slice(0, 5)) {
    console.error("      unauthenticated call:", c.replace(/\s+/g, " ").slice(0, 110));
  }

  check("withAuth() sends the token as a header, not a query parameter", /X-Drifterr-Token/.test(code) && !/[?&]token=/.test(code));
  check("panel pairs before its first poll", /ensureToken\(\)\s*\.then/.test(code));

  // The served dashboard is paired by substituting this exact literal; a rename
  // on either side would silently ship an unpairable page.
  const html = await readFile(join(UI_DIR, "index.html"), "utf8");
  check("index.html carries the token placeholder the server substitutes", html.includes("__DRIFTERR_CONTROL_TOKEN__"));
  const dashboardRs = await readFile(
    join(UI_DIR, "..", "..", "..", "crates", "proxy", "src", "dashboard.rs"),
    "utf8",
  );
  check("dashboard.rs substitutes that same placeholder", dashboardRs.includes("__DRIFTERR_CONTROL_TOKEN__"));
}

if (failures) {
  console.error(`\nconfig.test: ${failures} check(s) failed`);
  process.exit(1);
}
console.log("\nconfig.test: all checks passed");
