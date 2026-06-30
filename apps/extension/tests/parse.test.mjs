// Headless verification of the DOM parser (src/parse.js) against representative
// markup for each supported host. Verifies ordering, role mapping, session id,
// and model — the logic; the live selectors still need tuning on real pages.
//
// Run: npm test   (from apps/extension)

import { chromium } from "playwright";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const PARSE_JS = join(dirname(fileURLToPath(import.meta.url)), "..", "src", "parse.js");

let failures = 0;
function check(cond, msg) {
  if (cond) console.log("  ✓ " + msg);
  else {
    failures++;
    console.error("  ✗ " + msg);
  }
}

const CHATGPT_HTML = `
  <main>
    <article data-message-author-role="user"><div>Refactor in TS, no JS</div></article>
    <article data-message-author-role="assistant"><div>Sure, creating auth.js</div></article>
  </main>`;

const CLAUDE_HTML = `
  <div>
    <div data-testid="user-message">Use argon2, don't use bcrypt</div>
    <div class="font-claude-message">Okay, using argon2 for hashing.</div>
  </div>`;

const COPILOT_HTML = `
  <div>
    <div data-content="user-message">Keep the API server-side</div>
    <div data-content="ai-message">Got it, server-side only.</div>
  </div>`;

const PERPLEXITY_HTML = `
  <div>
    <div data-testid="user-query">Summarize this, cite sources</div>
    <div data-testid="answer">Here is the summary with citations.</div>
  </div>`;

async function main() {
  const browser = await chromium.launch(
    process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}
  );
  const page = await browser.newPage();
  await page.addScriptTag({ path: PARSE_JS });

  console.log("ChatGPT DOM:");
  const cg = await page.evaluate(
    ([html]) => {
      document.body.innerHTML = html;
      return window.DrifterrParse.extract({
        hostname: "chatgpt.com",
        doc: document,
        pathname: "/c/abc-123def",
      });
    },
    [CHATGPT_HTML]
  );
  check(cg.turns.length === 2, "extracts both turns");
  check(cg.turns[0].role === "user" && cg.turns[1].role === "assistant", "maps roles in order");
  check(cg.turns[1].content.includes("auth.js"), "captures assistant text");
  check(cg.model === "gpt-4o", "infers model from host");
  check(cg.sessionId === "abc-123def", "session id from URL path");

  console.log("Claude DOM:");
  const cl = await page.evaluate(
    ([html]) => {
      document.body.innerHTML = html;
      return window.DrifterrParse.extract({
        hostname: "claude.ai",
        doc: document,
        pathname: "/chat/xyz-987654",
      });
    },
    [CLAUDE_HTML]
  );
  check(cl.turns.length === 2, "extracts both turns");
  check(cl.turns[0].role === "user", "user-message → user");
  check(cl.turns[1].role === "assistant", "claude-message → assistant");
  check(cl.model === "claude-opus-4-x", "infers claude model");

  console.log("Copilot DOM:");
  const co = await page.evaluate(
    ([html]) => {
      document.body.innerHTML = html;
      return window.DrifterrParse.extract({ hostname: "copilot.microsoft.com", doc: document, pathname: "/" });
    },
    [COPILOT_HTML]
  );
  check(co.turns.length === 2, "extracts both turns");
  check(co.turns[0].role === "user" && co.turns[1].role === "assistant", "maps copilot roles");
  check(co.model === "copilot", "infers copilot model");

  console.log("Perplexity DOM:");
  const px = await page.evaluate(
    ([html]) => {
      document.body.innerHTML = html;
      return window.DrifterrParse.extract({ hostname: "www.perplexity.ai", doc: document, pathname: "/search/abc123" });
    },
    [PERPLEXITY_HTML]
  );
  check(px.turns.length === 2, "extracts both turns");
  check(px.turns[0].role === "user" && px.turns[1].role === "assistant", "maps perplexity roles");
  check(px.model === "perplexity", "infers perplexity model");

  console.log("Unknown host:");
  const none = await page.evaluate(() =>
    window.DrifterrParse.extract({ hostname: "example.com", doc: document, pathname: "/" })
  );
  check(none === null, "returns null on unsupported host");

  await browser.close();
  if (failures) {
    console.error(`\n${failures} check(s) failed`);
    process.exit(1);
  }
  console.log("\nAll parser checks passed.");
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
