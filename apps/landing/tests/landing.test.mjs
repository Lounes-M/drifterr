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
  // Also switch analytics off by default so no run posts events; the analytics
  // behaviour gets its own scenario below with the collector stubbed.
  const neutralizeAccounts = (page) =>
    page.addInitScript(() => {
      window.DRIFTERR_SUPABASE_URL = "";
      window.DRIFTERR_SUPABASE_ANON_KEY = "";
      window.DRIFTERR_ANALYTICS_URL = "";
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

  // --- honesty guards ---
  //
  // These exist because the page previously shipped invented social proof, an
  // uncomputable "52 min saved" stat, and a flagship demo built on a constraint
  // the engine cannot check. Each is cheap to re-add by accident and expensive
  // in credibility, so each gets a test.
  {
    const page = await (await browser.newContext({ userAgent: MAC_UA })).newPage();
    await neutralizeAccounts(page);
    await page.goto(url);
    console.log("honesty guards:");
    const html = await page.content();
    const text = await page.locator("body").innerText();

    check(!/52\s*min/i.test(text), "no invented time-saved stat");
    check((await page.locator(".tm").count()) === 0, "no testimonial cards");
    for (const name of ["Maya R.", "Devon K.", "Priya S.", "Tom A."]) {
      check(!text.includes(name), `no fabricated testimonial from ${name}`);
    }
    // "server-side" is not one of the 11 deterministic rule families in
    // crates/engine/src/infer.rs, so it must not be shown as a detected
    // constraint violation.
    check(
      !/constraint[^.]{0,40}server-side|server-side[^.]{0,20}(broken|violated)/i.test(text),
      "no demo claims a server-side constraint violation"
    );
    // The demos should name rules that really are checked.
    check(/no new dep/i.test(text), "demo uses the real no-new-deps rule");
    check(/package\.json/i.test(text), "demo uses the real protected-file rule");

    // Pricing must mirror crates/proxy/src/entitlement.rs.
    check(/Unlimited sessions/i.test(text), "Free advertises unlimited sessions");
    check(
      /Automatic re-anchor injection/i.test(text),
      "Pro lists auto re-anchor, the capability actually gated"
    );
    check(
      /Unlimited session history/i.test(text),
      "Pro lists unlimited history, the other gated capability"
    );
    check(
      !/Constraint tracking \+ alerts/i.test(text),
      "Pro no longer sells constraint tracking, which is free"
    );
    check((await page.locator(".tier .soon").count()) === 0, "no unbuilt features listed as tier contents");
    check(/no account/i.test(text), "the page says no account is required");
    check(!/github\.com/.test(html), "landing page exposes no github.com link");

    // Positioning guards. The universal-chatbot-layer framing was broader and weaker
    // than the product, and it is the easy thing to drift back into.
    check(/coding agent/i.test(text), "leads with coding agents");
    check(/Claude Code/.test(text), "names the zero-setup channel");
    check(
      (await page.locator(".marquee").count()) === 0,
      "no scrolling logo wall — it reads as 'we integrate with everything'"
    );
    check((await page.locator(".chan").count()) === 3, "channels stated plainly instead");
    // The web-UI channel must stay labelled beta until the extension is store-published
    // (see apps/extension/STORE_LISTING.md).
    check(
      await page.evaluate(() => {
        const beta = document.querySelector(".chan-tag.chan-beta");
        return !!beta && /beta/i.test(beta.textContent);
      }),
      "the unpublished browser channel is labelled beta"
    );
    check(
      /not store-published/i.test(text),
      "says plainly that the extension isn't on the stores"
    );
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

    // The unsigned-build warning must be on the page, not buried in docs: a user
    // who misses it hits "Drifterr is damaged and can't be opened" and leaves.
    const text = await page.locator("body").innerText();
    check(await page.locator("#firstrun").count() === 1, "first-launch section exists");
    check(/code-signed|unsigned/i.test(text), "states plainly that builds aren't signed");
    check(/right-click/i.test(text), "gives the macOS right-click → Open fix");
    check(/More info/i.test(text) && /Run anyway/i.test(text), "gives the SmartScreen fix");
    check(/chmod \+x/i.test(text), "gives the Linux AppImage fix");
    check((await page.locator("[data-os-block]").count()) === 3, "one block per platform");
    // A macOS visitor should have the macOS block promoted.
    check(
      (await page.locator('[data-os-block="mac"]').getAttribute("class")).includes("mine"),
      "the visitor's own platform is highlighted"
    );
    check(!/Free forever/i.test(text), "no 'free forever' claim that contradicts the tiers");
    check(/no account/i.test(text), "download page repeats that no account is needed");
  }

  // --- analytics: measures the funnel, identifies nobody ---
  {
    const ctx = await browser.newContext({ userAgent: MAC_UA });
    const page = await ctx.newPage();
    await page.addInitScript(() => {
      window.DRIFTERR_SUPABASE_URL = "";
      window.DRIFTERR_SUPABASE_ANON_KEY = "";
      window.DRIFTERR_ANALYTICS_URL = "/api/event";
    });
    // Capture what would be sent, rather than sending it.
    const sent = [];
    await page.route("**/api/event", (route) => {
      try { sent.push(JSON.parse(route.request().postData() || "{}")); } catch {}
      route.fulfill({ status: 204, body: "" });
    });
    await page.goto(url);
    console.log("analytics:");

    await page.waitForFunction(() => true);
    // sendBeacon bypasses page.route in some builds, so assert via the module's
    // own contract too — that's where the privacy guarantees actually live.
    const probe = await page.evaluate(async () => {
      const a = await import("/analytics.js");
      return {
        // Coarse page labels only — never a raw path or query string.
        home: a.pageLabel("/index.html"),
        dl: a.pageLabel("/download"),
        unknown: a.pageLabel("/blog/some-post?utm_source=x"),
        // Unknown events and unknown properties are dropped, not forwarded.
        rejectsUnknownEvent: a.track("exfiltrate_prompt", {}),
        // Free text and PII-shaped values never survive sanitization.
        cleaned: a.sanitize({
          os: "mac",
          plan: "pro",
          email: "someone@example.com",
          goal: "ship the billing API",
          url: "https://drifterr.app/?token=abc",
          index: 3,
        }),
        badEnum: a.sanitize({ os: "haiku-os", plan: "enterprise" }),
        bigIndex: a.sanitize({ index: 5000 }),
      };
    });
    check(probe.home === "home", "page label is coarse for the home page");
    check(probe.dl === "download", "page label is coarse for /download");
    check(probe.unknown === "other", "unknown paths collapse to 'other', dropping the query string");
    check(probe.rejectsUnknownEvent === false, "events outside the allowlist are refused");
    check(
      JSON.stringify(probe.cleaned) === JSON.stringify({ os: "mac", plan: "pro", index: 3 }),
      "sanitize keeps only allowlisted props — email/goal/url are dropped"
    );
    check(Object.keys(probe.badEnum).length === 0, "values outside the enum are dropped");
    check(Object.keys(probe.bigIndex).length === 0, "out-of-range numbers are dropped");

    // Nothing that reaches the wire may carry an identifier.
    const bodies = JSON.stringify(sent);
    check(!/uid|visitor|client_id|session_id/i.test(bodies), "no visitor identifier is sent");
    // And nothing may persist client-side state to build one from.
    const storage = await page.evaluate(() => ({
      cookies: document.cookie,
      keys: Object.keys(localStorage).filter((k) => /analytic|track|visitor|uid/i.test(k)),
    }));
    check(storage.cookies === "", "analytics sets no cookie");
    check(storage.keys.length === 0, "analytics writes no tracking key to localStorage");

    // Do Not Track is a hard stop.
    const dntCtx = await browser.newContext({ userAgent: MAC_UA });
    const dntPage = await dntCtx.newPage();
    await dntPage.addInitScript(() => {
      window.DRIFTERR_ANALYTICS_URL = "/api/event";
      Object.defineProperty(navigator, "doNotTrack", { get: () => "1" });
    });
    await dntPage.goto(url);
    const dntBlocked = await dntPage.evaluate(async () => {
      const a = await import("/analytics.js");
      return a.track("page_view") === false;
    });
    check(dntBlocked, "Do Not Track suppresses every event");
    await dntCtx.close();
    await ctx.close();
  }

  // --- proof page draws the app/website boundary ---
  {
    const page = await (await browser.newContext()).newPage();
    await neutralizeAccounts(page);
    await page.goto(`http://127.0.0.1:${port}/proof.html`);
    console.log("proof page:");
    const text = await page.locator("body").innerText();
    check(/The app sends nothing/i.test(text), "states the app sends nothing");
    check(/cookieless/i.test(text), "discloses the site analytics as cookieless");
    check(/Do Not Track/i.test(text), "documents the DNT/GPC opt-out");
    check(
      /no visitor identifier/i.test(text),
      "states there is no visitor identifier"
    );
    // Team sharing is the one thing the app can be asked to upload, so the proof page
    // must say so. "The app sends nothing" would otherwise be a claim the product no
    // longer honours — exactly the kind of gap this page exists to close.
    check(
      /sends nothing on its own/i.test(text),
      "qualifies the no-egress claim now that Team sharing exists"
    );
    check(
      /Team sharing/i.test(text) && /rule packs/i.test(text),
      "names what Team sharing uploads"
    );
    check(
      /offending spans?/i.test(text) && /session ids?/i.test(text),
      "names what Team sharing never uploads"
    );
    check(
      /exactly what would be shared/i.test(text),
      "points at the in-app payload preview, so the claim is checkable"
    );
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
