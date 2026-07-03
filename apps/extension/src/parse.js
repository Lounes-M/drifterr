// Drifterr browser channel — DOM → normalized turns.
//
// Per-host selectors extract the visible conversation. This is best-effort:
// the chat sites' DOMs change, so treat the selectors as a starting point to
// tune against the live pages. The *logic* (ordering, role mapping, session id,
// payload shape) is what the test pins down.
//
// Exposes `window.DrifterrParse.extract(opts)` so it works both as an injected
// content script (no args → uses the live document/location) and in tests
// (explicit `{ hostname, doc, pathname }`).

(function () {
  const CONFIGS = [
    {
      match: /(^|\.)chatgpt\.com$|openai\.com$/,
      items: "[data-message-author-role]",
      role: (el) => el.getAttribute("data-message-author-role"),
      model: "gpt-4o",
      composer: "#prompt-textarea, textarea[data-id], form textarea",
    },
    {
      match: /(^|\.)claude\.ai$/,
      items: '[data-testid="user-message"], .font-claude-message',
      role: (el) =>
        el.getAttribute("data-testid") === "user-message" ? "user" : "assistant",
      model: "claude-opus-4-x",
      composer: 'div[contenteditable="true"].ProseMirror, div[contenteditable="true"]',
    },
    {
      match: /(^|\.)gemini\.google\.com$/,
      items: "user-query, model-response",
      role: (el) => (el.tagName.toLowerCase() === "user-query" ? "user" : "assistant"),
      model: "gemini-1.5-pro",
      composer: 'rich-textarea .ql-editor, .ql-editor[contenteditable="true"], textarea',
    },
    {
      match: /(^|\.)copilot\.microsoft\.com$/,
      items: '[data-content="user-message"], [data-content="ai-message"]',
      role: (el) =>
        el.getAttribute("data-content") === "user-message" ? "user" : "assistant",
      model: "copilot",
      composer: "textarea#userInput, textarea",
    },
    {
      match: /(^|\.)perplexity\.ai$/,
      items: '[data-testid="user-query"], [data-testid="answer"]',
      role: (el) =>
        el.getAttribute("data-testid") === "user-query" ? "user" : "assistant",
      model: "perplexity",
      composer: 'textarea[placeholder], div[contenteditable="true"]',
    },
  ];

  function pickConfig(hostname) {
    return CONFIGS.find((c) => c.match.test(hostname || ""));
  }

  function textOf(el) {
    const t = el.innerText != null ? el.innerText : el.textContent || "";
    return t.replace(/\s+\n/g, "\n").trim();
  }

  function normalizeRole(r) {
    if (r === "assistant" || r === "model") return "assistant";
    if (r === "tool" || r === "function") return "tool";
    return "user";
  }

  function sessionIdFrom(pathname, turns) {
    const seg = (pathname || "")
      .split("/")
      .filter(Boolean)
      .pop();
    if (seg && seg.length >= 6) return seg;
    const first = turns.find((t) => t.role === "user");
    return "h" + simpleHash(first ? first.content : "default");
  }

  function simpleHash(s) {
    let h = 0x811c9dc5;
    for (let i = 0; i < s.length; i++) {
      h ^= s.charCodeAt(i);
      h = (h * 0x01000193) >>> 0;
    }
    return h.toString(16);
  }

  function extract(opts) {
    opts = opts || {};
    const doc = opts.doc || (typeof document !== "undefined" ? document : null);
    const hostname =
      opts.hostname ||
      (typeof location !== "undefined" ? location.hostname : "");
    const pathname =
      opts.pathname || (typeof location !== "undefined" ? location.pathname : "");
    if (!doc) return null;

    const cfg = pickConfig(hostname);
    if (!cfg) return null;

    const turns = [];
    for (const el of doc.querySelectorAll(cfg.items)) {
      const content = textOf(el);
      if (!content) continue;
      turns.push({ role: normalizeRole(cfg.role(el)), content });
    }
    if (!turns.length) return null;

    return {
      sessionId: sessionIdFrom(pathname, turns),
      model: cfg.model,
      turns,
    };
  }

  /// Inject re-anchor text into the page's chat composer — the browser-channel
  /// equivalent of "one-click re-anchor". Best-effort: finds the host's composer,
  /// prepends `text` to whatever's there, and dispatches an `input` event so the
  /// site's framework registers the change. Returns true if it found a composer.
  function inject(text, opts) {
    opts = opts || {};
    const doc = opts.doc || (typeof document !== "undefined" ? document : null);
    const hostname =
      opts.hostname || (typeof location !== "undefined" ? location.hostname : "");
    if (!doc || !text) return false;
    const cfg = pickConfig(hostname);
    if (!cfg || !cfg.composer) return false;
    const box = doc.querySelector(cfg.composer);
    if (!box) return false;

    const isField = box.tagName === "TEXTAREA" || box.tagName === "INPUT";
    const existing = (isField ? box.value : box.textContent) || "";
    const combined = existing.trim() ? text + "\n\n" + existing : text;
    if (isField) {
      box.value = combined;
    } else {
      box.textContent = combined;
    }
    try {
      box.dispatchEvent(new Event("input", { bubbles: true }));
    } catch (_e) {
      /* no Event constructor in some test envs */
    }
    if (typeof box.focus === "function") box.focus();
    return true;
  }

  const api = { extract, inject };
  if (typeof window !== "undefined") window.DrifterrParse = api;
  if (typeof globalThis !== "undefined") globalThis.DrifterrParse = api;
})();
