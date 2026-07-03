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

  console.log("Resilience (fallback / dedupe / visibility):");
  // Fallback variant: Gemini's primary custom elements are absent, but the
  // class-based fallback still recovers the turns.
  const fb = await page.evaluate(() => {
    document.body.innerHTML =
      '<div class="query-text">what is drift</div><div class="model-response-text">a divergence from intent</div>';
    return window.DrifterrParse.extract({ hostname: "gemini.google.com", doc: document, pathname: "/app/x" });
  });
  check(fb && fb.turns.length === 2, "falls back to the secondary selector when the primary matches nothing");
  check(fb.turns[0].role === "user" && fb.turns[1].role === "assistant", "fallback maps roles by class");

  // Dedupe: a repeated consecutive identical turn (streaming re-render) collapses.
  const dd = await page.evaluate(() => {
    document.body.innerHTML =
      '<div data-message-author-role="user">hi</div>' +
      '<div data-message-author-role="assistant">hello</div>' +
      '<div data-message-author-role="assistant">hello</div>';
    return window.DrifterrParse.extract({ hostname: "chatgpt.com", doc: document, pathname: "/" });
  });
  check(dd.turns.length === 2, "collapses consecutive identical turns");

  // Visibility: a display:none node is skipped.
  const vis = await page.evaluate(() => {
    document.body.innerHTML =
      '<div data-message-author-role="user" style="display:none">ghost draft</div>' +
      '<div data-message-author-role="user">real question</div>' +
      '<div data-message-author-role="assistant">real answer</div>';
    return window.DrifterrParse.extract({ hostname: "chatgpt.com", doc: document, pathname: "/" });
  });
  check(vis.turns.length === 2, "skips hidden (display:none) nodes");
  check(vis.turns[0].content === "real question", "keeps only the visible user turn");

  console.log("Re-anchor inject:");
  // Textarea composer (ChatGPT-style): prepends the preamble to whatever's there.
  const injField = await page.evaluate(() => {
    document.body.innerHTML = '<textarea id="prompt-textarea">my draft</textarea>';
    const ok = window.DrifterrParse.inject("[re-anchor] stay in TS", { hostname: "chatgpt.com", doc: document });
    return { ok, value: document.getElementById("prompt-textarea").value };
  });
  check(injField.ok === true, "inject finds the textarea composer");
  check(injField.value.startsWith("[re-anchor] stay in TS"), "preamble is prepended");
  check(injField.value.includes("my draft"), "existing draft is preserved");

  // Contenteditable composer (Claude-style) into an empty box.
  const injCE = await page.evaluate(() => {
    document.body.innerHTML = '<div contenteditable="true" class="ProseMirror"></div>';
    const ok = window.DrifterrParse.inject("[re-anchor] honor constraints", { hostname: "claude.ai", doc: document });
    return { ok, text: document.querySelector(".ProseMirror").textContent };
  });
  check(injCE.ok === true, "inject finds the contenteditable composer");
  check(injCE.text === "[re-anchor] honor constraints", "sets contenteditable text");

  // No composer present → returns false, never throws.
  const injNone = await page.evaluate(() => {
    document.body.innerHTML = "<div></div>";
    return window.DrifterrParse.inject("x", { hostname: "chatgpt.com", doc: document });
  });
  check(injNone === false, "inject returns false when no composer is found");

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
