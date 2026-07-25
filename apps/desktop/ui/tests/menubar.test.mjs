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
    // Mark onboarding as already seen so the first-run tour doesn't overlay the
    // panel during these checks (the tour has its own scenario below).
    try { localStorage.setItem("drifterr_onboarded", "1"); } catch (_e) {}
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
  // Copy preamble → clipboard holds the short in-thread reminder, not the snapshot.
  await page.locator("#reanchor-copy-preamble").click();
  await page.waitForFunction(() => document.getElementById("reanchor-copy-preamble").textContent === "Copied!");
  const clipP = await page.evaluate(() => navigator.clipboard.readText());
  check(clipP.includes("Binding constraints"), "Copy preamble copies the preamble");
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
  check(await page.locator("#upgrade-nudge").isVisible(), "Free shows the upgrade nudge");
  check((await page.locator("#plan-pill").textContent()) === "Free", "plan pill reads Free");
  // Regression: the nudge is driven by the proxy entitlement, which knows nothing
  // about whether accounts are configured — so its button must be wired either
  // way, or Free users get a visible button that does nothing. (Accounts are
  // neutralized in this context, which is exactly the case that used to break.)
  check(
    await page.evaluate(() => {
      const b = document.getElementById("upgrade-btn");
      // A wired listener calls window.open; stub it and see if it fires.
      let opened = false;
      const real = window.open;
      window.open = () => { opened = true; };
      b.click();
      window.open = real;
      return opened;
    }),
    "upgrade button works with accounts unconfigured"
  );
  scenario = { current: baseCur, sessions: [], entitlement: { plan: "pro", driftMap: true }, sessionsLocked: 0 };
  await page.waitForFunction(() => !document.getElementById("drift-map-section").classList.contains("locked"), null, { timeout: 5000 });
  check(!(await page.locator("#map-lock").isVisible()), "Pro unlocks the drift map");
  check(!(await page.locator("#upgrade-nudge").isVisible()), "Pro hides the upgrade nudge");

  console.log("WEEKLY report:");
  await page.route("**/report*", (route) =>
    route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        markdown: "# Drifterr — last 7 days\n\n- **3** sessions tracked\n- **9** flags raised\n",
        flags: 9,
        sessions: 3,
        reanchors: 2,
        quietWeek: false,
      }),
    })
  );
  await page.locator("#weekly-btn").click();
  await page.waitForFunction(() => document.getElementById("weekly-text").textContent.includes("9"), null, { timeout: 5000 });
  check(await page.locator("#weekly").isVisible(), "weekly report opens");
  check((await page.locator("#weekly-text").textContent()).includes("flags raised"), "renders the generated markdown");
  await page.locator("#weekly-btn").click();
  check(await page.locator("#weekly").isHidden(), "second click closes it");
  // No local database must read as "no history", never as "nothing drifted".
  await page.unroute("**/report*");
  await page.route("**/report*", (route) => route.fulfill({ status: 503, body: "no local database" }));
  await page.locator("#weekly-btn").click();
  await page.waitForFunction(() => document.getElementById("weekly-text").textContent.includes("No local database"), null, { timeout: 5000 });
  check(
    (await page.locator("#weekly-text").textContent()).includes("no history"),
    "missing DB is explained, not shown as an empty report"
  );
  await page.locator("#weekly-btn").click();

  console.log("RE-ANCHOR outcome (did it hold?):");
  const withMark = (mark) => ({
    current: { ...baseCur, reanchor: mark },
    sessions: [],
    entitlement: { plan: "pro", driftMap: true },
    sessionsLocked: 0,
  });
  // Held: two quiet turns since re-anchoring.
  scenario = withMark({ atTurn: 4, signal: "constraint", constraintId: "c1", heldTurns: 3 });
  await page.waitForFunction(() => !document.getElementById("reanchor-outcome").hidden, null, { timeout: 5000 });
  check((await page.locator("#ro-badge").textContent()) === "Re-anchor held", "reports a held re-anchor");
  check((await page.locator("#ro-text").textContent()).includes("3 turns"), "names how many turns it held");
  check((await page.locator("#reanchor-outcome").getAttribute("class")).includes("held"), "styled as held");
  // Broke: the same cause came back.
  scenario = withMark({ atTurn: 4, signal: "constraint", constraintId: "c1", heldTurns: 0, brokeAgainAtTurn: 6 });
  await page.waitForFunction(() => document.getElementById("ro-badge").textContent === "Didn't hold", null, { timeout: 5000 });
  check((await page.locator("#ro-text").textContent()).includes("turn 7"), "names the turn it broke again (1-based)");
  check((await page.locator("#reanchor-outcome").getAttribute("class")).includes("broke"), "styled as broken");
  // Undecided must NOT read as success — one quiet turn is not evidence.
  scenario = withMark({ atTurn: 4, signal: "constraint", constraintId: "c1", heldTurns: 1 });
  await page.waitForFunction(() => document.getElementById("ro-badge").textContent === "Checking", null, { timeout: 5000 });
  check((await page.locator("#reanchor-outcome").getAttribute("class")).includes("pending"), "undecided is neutral, not a win");
  // No re-anchor yet ⇒ nothing shown.
  scenario = withMark(undefined);
  await page.waitForFunction(() => document.getElementById("reanchor-outcome").hidden, null, { timeout: 5000 });
  check(await page.locator("#reanchor-outcome").isHidden(), "hidden when no re-anchor happened");

  console.log("TRIAL (local first-run Pro):");
  scenario = { current: baseCur, sessions: [], entitlement: { plan: "trial", driftMap: true, trialDaysLeft: 12 }, sessionsLocked: 0 };
  await page.waitForFunction(() => document.getElementById("plan-pill").textContent.includes("trial"), null, { timeout: 5000 });
  check((await page.locator("#plan-pill").textContent()) === "Pro trial · 12d", "trial pill shows the countdown");
  check(!(await page.locator("#map-lock").isVisible()), "trial unlocks the drift map");
  check(!(await page.locator("#upgrade-nudge").isVisible()), "no upgrade nudge during the trial");
  check(!(await page.locator("#trial-ending").isVisible()), "no ending warning with 12 days left");
  // Inside the last three days the panel names what is about to switch off.
  scenario = { current: baseCur, sessions: [], entitlement: { plan: "trial", driftMap: true, trialDaysLeft: 2 }, sessionsLocked: 0 };
  await page.waitForFunction(() => !document.getElementById("trial-ending").hidden, null, { timeout: 5000 });
  check(await page.locator("#trial-ending").isVisible(), "warns when the trial is nearly over");
  check((await page.locator("#trial-ending-text").textContent()).includes("2 days"), "names the days remaining");
  check((await page.locator("#trial-ending-text").textContent()).includes("detection keeps running"), "reassures that detection survives");

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
  await page.route("**/providers*", (route) =>
    route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        current: "openrouter",
        providers: [
          { id: "openrouter", label: "OpenRouter" },
          { id: "openai", label: "OpenAI" },
          { id: "gemini", label: "Google Gemini" },
        ],
      }),
    })
  );
  let switchedTo = null;
  await page.route("**/provider", (route) => {
    switchedTo = JSON.parse(route.request().postData() || "{}").id;
    route.fulfill({ contentType: "application/json", body: JSON.stringify({ provider: switchedTo }) });
  });
  // Auto re-anchor toggle: starts off+allowed; POST flips it on.
  let autoOn = false;
  await page.route("**/auto-reanchor*", (route) => {
    if (route.request().method() === "POST") autoOn = JSON.parse(route.request().postData() || "{}").on;
    route.fulfill({ contentType: "application/json", body: JSON.stringify({ on: autoOn, allowed: true, effective: autoOn }) });
  });
  // Judge: starts disabled; POST with a key enables it (key never sent back).
  let judgePosted = null;
  await page.route("**/judge*", (route) => {
    if (route.request().method() === "POST") judgePosted = JSON.parse(route.request().postData() || "{}");
    const on = !!(judgePosted && judgePosted.apiKey);
    route.fulfill({ contentType: "application/json", body: JSON.stringify({ enabled: on, label: on ? (judgePosted.model || "openai/gpt-4o-mini") : "disabled" }) });
  });
  // Auto-intent: judge ready, starts off; POST flips it on.
  let autoIntentOn = false;
  await page.route("**/auto-intent*", (route) => {
    if (route.request().method() === "POST") autoIntentOn = JSON.parse(route.request().postData() || "{}").on;
    route.fulfill({ contentType: "application/json", body: JSON.stringify({ on: autoIntentOn, judgeReady: true }) });
  });
  // Prefs: Do Not Disturb starts off; POST flips it.
  let dndMuted = false;
  await page.route("**/prefs*", (route) => {
    if (route.request().method() === "POST") dndMuted = JSON.parse(route.request().postData() || "{}").notificationsMuted;
    route.fulfill({ contentType: "application/json", body: JSON.stringify({ notificationsMuted: dndMuted }) });
  });
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
  check((await page.locator("#cfg-storage").textContent()) === "In-memory", "settings shows storage mode");
  check((await page.locator("#cfg-judge").textContent()).includes("gpt-4o-mini"), "settings shows judge model");

  console.log("AUTO RE-ANCHOR toggle:");
  await page.waitForFunction(() => document.getElementById("auto-reanchor-label").textContent === "Off");
  check(!(await page.locator("#auto-reanchor-toggle").isChecked()), "auto re-anchor starts off");
  await page.locator("#auto-reanchor-toggle + .toggle-track").click();
  await page.waitForFunction(() => document.getElementById("auto-reanchor-label").textContent === "On");
  check(autoOn === true, "toggling posts on=true to the proxy");
  check(await page.locator("#auto-reanchor-toggle").isChecked(), "toggle reflects the on state");

  console.log("JUDGE config:");
  await page.waitForFunction(() => document.getElementById("judge-status").textContent === "Off");
  check((await page.locator("#judge-status").textContent()) === "Off", "judge starts off");
  await page.locator("#judge-key").fill("sk-or-secret");
  await page.locator("#judge-model").fill("openai/gpt-4o-mini");
  await page.locator("#judge-save").click();
  await page.waitForFunction(() => document.getElementById("judge-status").textContent.startsWith("On"));
  check(judgePosted && judgePosted.apiKey === "sk-or-secret", "save posts the api key to the proxy");
  check((await page.locator("#judge-key").inputValue()) === "", "key field is cleared after save");
  check((await page.locator("#judge-status").textContent()).includes("gpt-4o-mini"), "status shows the active model");

  console.log("AUTO-INTENT toggle:");
  await page.waitForFunction(() => document.getElementById("auto-intent-label").textContent === "Off");
  check(!(await page.locator("#auto-intent-toggle").isChecked()), "auto-intent starts off");
  check(await page.locator("#auto-intent-hint").isHidden(), "no 'needs judge' hint when judge is ready");
  await page.locator("#auto-intent-toggle + .toggle-track").click();
  await page.waitForFunction(() => document.getElementById("auto-intent-label").textContent === "On");
  check(autoIntentOn === true, "toggling posts on=true to the proxy");

  console.log("DO NOT DISTURB toggle:");
  await page.waitForFunction(() => document.getElementById("dnd-label").textContent === "Off");
  check(!(await page.locator("#dnd-toggle").isChecked()), "DND starts off");
  await page.locator("#dnd-toggle + .toggle-track").click();
  await page.waitForFunction(() => document.getElementById("dnd-label").textContent === "On");
  check(dndMuted === true, "toggling DND posts notificationsMuted=true");
  check((await page.locator("#upd-version").textContent()).includes("0.0.1"), "updates row shows the running version");

  console.log("PROVIDER selector:");
  await page.waitForFunction(() => document.querySelectorAll("#provider-select .provider-pill").length === 3);
  check((await page.locator("#provider-select .provider-pill").count()) === 3, "provider pills render");
  check(
    (await page.locator("#provider-select .provider-pill.active").textContent()) === "OpenRouter",
    "current provider is marked active"
  );
  await page.locator('#provider-select .provider-pill[data-id="gemini"]').click();
  await page.waitForFunction(() => {
    const a = document.querySelector("#provider-select .provider-pill.active");
    return a && a.dataset.id === "gemini";
  });
  check(switchedTo === "gemini", "clicking a provider POSTs the switch");
  check(
    (await page.locator("#provider-select .provider-pill.active").textContent()) === "Google Gemini",
    "selection moves the active state"
  );

  await page.locator("#gear").click();
  check(!(await page.locator("#settings").isVisible()), "gear closes settings again");

  console.log("OFFLINE scenario:");
  await page.unroute("**/status*");
  await page.route("**/status*", (route) => route.abort());
  await page.waitForFunction(() => !document.getElementById("error").hidden, null, { timeout: 5000 });
  check(await page.locator("#error").isVisible(), "shows offline error when API unreachable");

  console.log("ONBOARDING (first run):");
  {
    // Fresh context → no "onboarded" flag → the tour should appear and gate the
    // panel until completed.
    const octx = await browser.newContext();
    const op = await octx.newPage();
    await op.addInitScript(() => {
      window.DRIFTERR_SUPABASE_URL = "";
      window.DRIFTERR_SUPABASE_ANON_KEY = "";
    });
    await op.route("**/providers*", (route) =>
      route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ current: "openrouter", providers: [{ id: "openrouter", label: "OpenRouter" }, { id: "openai", label: "OpenAI" }] }),
      })
    );
    await op.route("**/status*", (route) => route.fulfill({ contentType: "application/json", body: JSON.stringify({ current: null, sessions: [] }) }));
    await op.goto(url);
    await op.waitForFunction(() => !document.getElementById("onboarding").hidden);
    check(await op.locator("#onboarding").isVisible(), "first run shows the onboarding tour");
    check((await op.locator("#onb-provider-select .provider-pill").count()) > 0, "tour embeds the provider selector");
    // Walk to the end: Next ×4 → Get started (5 steps: welcome, provider, tool,
    // intent, ready).
    for (let i = 0; i < 4; i++) await op.locator("#onb-next").click();
    check((await op.locator("#onb-next").textContent()) === "Get started", "last step shows Get started");
    await op.locator("#onb-next").click();
    await op.waitForFunction(() => document.getElementById("onboarding").hidden);
    check(!(await op.locator("#onboarding").isVisible()), "finishing dismisses the tour");
    // Reload → it stays dismissed (persisted).
    await op.reload();
    await op.waitForFunction(() => typeof document.getElementById("onboarding") !== "undefined");
    check(await op.locator("#onboarding").isHidden(), "tour does not reappear after completion");
    await octx.close();
  }

  console.log("INTENT (declare / edit):");
  {
    const ictx = await browser.newContext();
    const ip = await ictx.newPage();
    await ip.addInitScript(() => {
      window.DRIFTERR_SUPABASE_URL = "";
      window.DRIFTERR_SUPABASE_ANON_KEY = "";
      try { localStorage.setItem("drifterr_onboarded", "1"); } catch (_e) {}
    });
    await ip.route("**/status*", (route) => route.fulfill({ contentType: "application/json", body: JSON.stringify(GREEN) }));
    // Stateful intent so polls stay consistent with edits/retires.
    let intentState = {
      goal: "Ship the billing API",
      constraints: [
        { id: "c1", text: "TypeScript only, no JS", kind: "tech", checkable: "deterministic", active: true },
        { id: "c2", text: "Keep the tone formal", kind: "tone", checkable: "judge", active: true },
      ],
      pending: false,
    };
    let posted = null;
    await ip.route("**/intent*", (route) => {
      const req = route.request();
      if (req.method() === "POST") {
        posted = JSON.parse(req.postData() || "{}");
        intentState = {
          goal: posted.goal,
          constraints: (posted.constraints || []).map((t, i) => ({
            id: "c" + (i + 1), text: t, kind: "other",
            checkable: /no js|typescript/i.test(t) ? "deterministic" : "judge", active: true,
          })),
          pending: false,
        };
      }
      route.fulfill({ contentType: "application/json", body: JSON.stringify(intentState) });
    });
    let retired = null;
    await ip.route("**/intent/retire*", (route) => {
      retired = JSON.parse(route.request().postData() || "{}");
      intentState = { ...intentState, constraints: intentState.constraints.filter((c) => c.id !== retired.id) };
      route.fulfill({ contentType: "application/json", body: JSON.stringify(intentState) });
    });
    await ip.goto(url);
    await ip.waitForFunction(() => document.getElementById("intent-goal")?.textContent?.includes("billing"));
    check((await ip.locator("#intent-goal").textContent()).includes("Ship the billing API"), "intent card shows the goal");
    check((await ip.locator(".intent-badge.hard").count()) === 1, "deterministic constraint shows a Hard badge");
    check((await ip.locator(".intent-badge.soft").count()) === 1, "judge constraint shows a Soft badge");

    // Retire the first constraint via its × button.
    check((await ip.locator(".intent-remove").count()) === 2, "each constraint has a remove button");
    await ip.locator(".intent-remove").first().click();
    await ip.waitForFunction(() => document.querySelectorAll("#intent-constraints .intent-constraint").length === 1);
    check(retired && retired.id === "c1", "remove posts the constraint id to /intent/retire");

    // Edit → change goal → save → POST body carries the new intent.
    await ip.locator("#intent-edit").click();
    check(await ip.locator("#intent-editor").isVisible(), "Edit opens the editor prefilled");
    await ip.locator("#intent-goal-input").fill("Refactor auth, no JS files");
    await ip.locator("#intent-constraints-input").fill("no JS\nbe concise");
    await ip.locator("#intent-save").click();
    await ip.waitForFunction(() => document.getElementById("intent-editor").hidden);
    check(posted && posted.goal === "Refactor auth, no JS files", "save POSTs the edited goal");
    check(posted && posted.constraints.length === 2, "save POSTs the constraint lines");
    await ictx.close();
  }

  console.log("INTENT SHIFT (Auto-intent pivot prompt):");
  {
    const sctx = await browser.newContext();
    const sp = await sctx.newPage();
    await sp.addInitScript(() => {
      window.DRIFTERR_SUPABASE_URL = ""; window.DRIFTERR_SUPABASE_ANON_KEY = "";
      try { localStorage.setItem("drifterr_onboarded", "1"); } catch (_e) {}
    });
    const withShift = JSON.parse(JSON.stringify(GREEN));
    withShift.current.intentShift = { from: "Refactor the auth module", to: "Build a React analytics dashboard" };
    await sp.route("**/status*", (route) => route.fulfill({ contentType: "application/json", body: JSON.stringify(withShift) }));
    await sp.route("**/intent*", (route) => route.fulfill({ contentType: "application/json", body: JSON.stringify({ goal: "Refactor the auth module", constraints: [], pending: false }) }));
    let shiftPosted = null;
    await sp.route("**/intent-shift*", (route) => { shiftPosted = JSON.parse(route.request().postData() || "{}"); route.fulfill({ contentType: "application/json", body: JSON.stringify({ goal: "Build a React analytics dashboard", constraints: [], pending: false }) }); });
    await sp.goto(url);
    await sp.waitForFunction(() => !document.getElementById("intent-shift").hidden);
    check(await sp.locator("#intent-shift").isVisible(), "goal-shift banner appears when a shift is pending");
    check((await sp.locator("#ishift-to").textContent()) === "Build a React analytics dashboard", "banner shows the proposed new goal");
    await sp.locator("#ishift-accept").click();
    await sp.waitForTimeout(150);
    check(shiftPosted && shiftPosted.accept === true, "Accept posts accept=true to /intent-shift");
    await sctx.close();
  }

  console.log("ACTIVITY journal:");
  {
    const actx = await browser.newContext();
    const ap = await actx.newPage();
    await ap.addInitScript(() => {
      window.DRIFTERR_SUPABASE_URL = ""; window.DRIFTERR_SUPABASE_ANON_KEY = "";
      try { localStorage.setItem("drifterr_onboarded", "1"); } catch (_e) {}
    });
    await ap.route("**/status*", (route) => route.fulfill({ contentType: "application/json", body: JSON.stringify(RED) }));
    await ap.route("**/journal*", (route) =>
      route.fulfill({
        contentType: "application/json",
        body: JSON.stringify([
          { signal: "constraint", state: "red", detail: "created auth.js", constraintId: "c1", span: "auth.js", turn: 3 },
          { signal: "goal_alignment", state: "amber", detail: "drifting from the goal", turn: 2 },
        ]),
      })
    );
    await ap.goto(url);
    await ap.waitForFunction(() => document.querySelectorAll("#activity-list .activity-row").length === 2);
    check((await ap.locator("#activity-list .activity-row").count()) === 2, "activity lists recent flags");
    check((await ap.locator(".activity-name").first().textContent()) === "Constraints", "names the signal");
    check((await ap.locator(".activity-turn").first().textContent()) === "turn 4", "shows the 1-based turn");
    check((await ap.locator("#activity-list .mini.red").count()) === 1, "flag dot reflects state");
    await actx.close();
  }

  console.log("SESSION REPORT export:");
  {
    const rctx = await browser.newContext({ permissions: ["clipboard-read", "clipboard-write"] });
    const rp = await rctx.newPage();
    await rp.addInitScript(() => {
      window.DRIFTERR_SUPABASE_URL = ""; window.DRIFTERR_SUPABASE_ANON_KEY = "";
      try { localStorage.setItem("drifterr_onboarded", "1"); } catch (_e) {}
    });
    await rp.route("**/status*", (route) => route.fulfill({ contentType: "application/json", body: JSON.stringify(RED) }));
    await rp.route("**/intent*", (route) => route.fulfill({ contentType: "application/json", body: JSON.stringify({ goal: "Ship the billing API", constraints: [{ id: "c1", text: "TypeScript only, no JS", kind: "tech", checkable: "deterministic", active: true }], pending: false }) }));
    await rp.route("**/journal*", (route) => route.fulfill({ contentType: "application/json", body: JSON.stringify([{ signal: "constraint", state: "red", detail: "created auth.js", turn: 3 }]) }));
    await rp.goto(url);
    await rp.waitForFunction(() => !document.getElementById("report-copy").hidden);
    await rp.locator("#report-copy").click();
    await rp.waitForFunction(() => document.getElementById("report-copy").textContent === "Copied!");
    const report = await rp.evaluate(() => navigator.clipboard.readText());
    check(report.includes("# Drifterr session report"), "report has a title");
    check(report.includes("Ship the billing API"), "report includes the goal");
    check(report.includes("created auth.js"), "report includes the activity journal");
    await rctx.close();
  }

  console.log("HISTORY view:");
  {
    const hctx = await browser.newContext();
    const hp = await hctx.newPage();
    await hp.addInitScript(() => {
      window.DRIFTERR_SUPABASE_URL = "";
      window.DRIFTERR_SUPABASE_ANON_KEY = "";
      try { localStorage.setItem("drifterr_onboarded", "1"); } catch (_e) {}
    });
    await hp.route("**/status*", (route) => route.fulfill({ contentType: "application/json", body: JSON.stringify(GREEN) }));
    await hp.route("**/history*", (route) =>
      route.fulfill({
        contentType: "application/json",
        body: JSON.stringify([
          { sessionId: "s2", model: "gpt-4o", goal: "Ship billing API", state: "red", turns: 12, lastActivity: Date.now() - 3600000 },
          { sessionId: "s1", model: "claude-opus-4-x", goal: "", state: "green", turns: 3, lastActivity: Date.now() - 86400000 },
        ]),
      })
    );
    await hp.goto(url);
    await hp.locator("#history-btn").click();
    await hp.waitForFunction(() => document.querySelectorAll("#history-list .history-row").length === 2);
    check((await hp.locator("#history-list .history-row").count()) === 2, "history lists past sessions");
    check((await hp.locator(".history-goal").first().textContent()) === "Ship billing API", "shows the session goal");
    check((await hp.locator(".history-goal.untitled").count()) === 1, "goalless session shows as Untitled");
    check((await hp.locator(".history-row .dot.red").count()) === 1, "state dot reflects the session state");
    await hctx.close();
  }

  // --- accounts configured, but signed out ---------------------------------
  //
  // The regression this guards: an auth gate that hides the panel body. Drift
  // detection is local, so a signed-out user must get the *whole* app. Only the
  // Account block should reflect the anonymous state.
  console.log("\nSigned-out panel (accounts configured):");
  {
    const actx = await browser.newContext();
    const ap = await actx.newPage();
    await ap.addInitScript(() => {
      // A plausible-looking config so `configured` is true and the accounts code
      // path actually runs...
      window.DRIFTERR_SUPABASE_URL = "https://example.supabase.co";
      window.DRIFTERR_SUPABASE_ANON_KEY = "anon-test-key";
      try { localStorage.setItem("drifterr_onboarded", "1"); } catch (_e) {}
    });
    // ...but stub the CDN import so the run stays hermetic: no session, no network.
    await ap.route("**/esm.sh/**", (route) =>
      route.fulfill({
        contentType: "text/javascript",
        body: "export const createClient = () => ({ auth: { getUser: async () => ({ data: { user: null } }), onAuthStateChange: () => {} } });",
      })
    );
    await ap.route("**/status*", (route) => route.fulfill({ contentType: "application/json", body: JSON.stringify(RED) }));
    await ap.goto(url);

    await ap.waitForFunction(() => document.getElementById("state-label").textContent === "Drifting");
    check(await ap.locator("#app-body").isVisible(), "panel body is visible while signed out");
    check(!(await ap.locator("#gate").isVisible()), "sign-in sheet stays closed");
    check(await ap.locator("#trigger").isVisible(), "drift trigger still renders signed out");
    check(await ap.locator("#reanchor-btn").isVisible(), "re-anchor is available signed out");

    // The sheet is reachable on demand from Settings → Account, and dismissible.
    await ap.locator("#gear").click();
    await ap.waitForSelector("#acct-anon", { state: "visible" });
    check(await ap.locator("#acct-anon").isVisible(), "Account block shows the signed-out state");
    check(!(await ap.locator("#acct-user").isVisible()), "signed-in details stay hidden");
    await ap.locator("#acct-signin").click();
    check(await ap.locator("#gate").isVisible(), "sign-in sheet opens on request");
    await ap.locator("#gate-dismiss").click();
    check(!(await ap.locator("#gate").isVisible()), "sign-in sheet is dismissible");
    check(await ap.locator("#app-body").isVisible(), "panel body survives dismissing the sheet");
    await actx.close();
  }

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
