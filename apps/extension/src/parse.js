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
      model: "gpt-4o",
      composer: "#prompt-textarea, textarea[data-id], form textarea",
      // `data-message-author-role` is OpenAI's own semantic attribute — stable.
      // Fallback reads the role off the nested attribute if the wrapper changes.
      variants: [
        { items: "[data-message-author-role]", role: (el) => el.getAttribute("data-message-author-role") },
        {
          items: "[data-testid^='conversation-turn'], article[data-testid]",
          role: (el) => {
            const inner = el.querySelector("[data-message-author-role]");
            return inner ? inner.getAttribute("data-message-author-role") : "user";
          },
        },
      ],
    },
    {
      match: /(^|\.)claude\.ai$/,
      model: "claude-opus-4-x",
      composer: 'div[contenteditable="true"].ProseMirror, div[contenteditable="true"]',
      variants: [
        {
          items: '[data-testid="user-message"], .font-claude-message',
          role: (el) => (el.getAttribute("data-testid") === "user-message" ? "user" : "assistant"),
        },
        {
          // Fallback: user turns keep the testid; assistant turns are streamed.
          items: '[data-testid="user-message"], [data-is-streaming], .font-claude-message',
          role: (el) => (el.getAttribute("data-testid") === "user-message" ? "user" : "assistant"),
        },
      ],
    },
    {
      match: /(^|\.)gemini\.google\.com$/,
      model: "gemini-1.5-pro",
      composer: 'rich-textarea .ql-editor, .ql-editor[contenteditable="true"], textarea',
      variants: [
        { items: "user-query, model-response", role: (el) => (el.tagName.toLowerCase() === "user-query" ? "user" : "assistant") },
        { items: ".query-text, .model-response-text, message-content", role: (el) => (el.classList.contains("query-text") ? "user" : "assistant") },
      ],
    },
    {
      match: /(^|\.)copilot\.microsoft\.com$/,
      model: "copilot",
      composer: "textarea#userInput, textarea",
      variants: [
        {
          items: '[data-content="user-message"], [data-content="ai-message"]',
          role: (el) => (el.getAttribute("data-content") === "user-message" ? "user" : "assistant"),
        },
      ],
    },
    {
      match: /(^|\.)perplexity\.ai$/,
      model: "perplexity",
      composer: 'textarea[placeholder], div[contenteditable="true"]',
      variants: [
        {
          items: '[data-testid="user-query"], [data-testid="answer"]',
          role: (el) => (el.getAttribute("data-testid") === "user-query" ? "user" : "assistant"),
        },
      ],
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

  /// Is `el` actually rendered? Skips display:none / hidden template nodes so we
  /// don't scrape off-screen duplicates. Lenient: anything we can't measure is
  /// treated as visible (never drop a real turn over a layout quirk).
  function isVisible(el) {
    if (el.hidden) return false;
    const win = el.ownerDocument && el.ownerDocument.defaultView;
    if (!win || typeof win.getComputedStyle !== "function") return true;
    const s = win.getComputedStyle(el);
    return s.display !== "none" && s.visibility !== "hidden";
  }

  /// Run one selector variant over the document → turns (visible, non-empty).
  function turnsForVariant(doc, variant) {
    const out = [];
    let els;
    try {
      els = doc.querySelectorAll(variant.items);
    } catch (_e) {
      return out; // an unsupported selector (e.g. :has on old engines) just fails soft
    }
    for (const el of els) {
      if (!isVisible(el)) continue;
      const content = textOf(el);
      if (!content) continue;
      const role = normalizeRole(variant.role(el));
      // Collapse consecutive identical turns (streaming can surface a node twice).
      const last = out[out.length - 1];
      if (last && last.role === role && last.content === content) continue;
      out.push({ role, content });
    }
    return out;
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

    // Try each selector variant in order; the first that yields turns wins. This
    // is the resilience layer — if a site changes the DOM under the primary
    // selector, a fallback can still recover the conversation.
    let turns = [];
    for (const variant of cfg.variants) {
      turns = turnsForVariant(doc, variant);
      if (turns.length) break;
    }
    if (!turns.length) return null;

    return {
      sessionId: sessionIdFrom(pathname, turns),
      model: cfg.model,
      turns,
    };
  }

  /// Why did extraction produce nothing?
  ///
  /// This exists because `extract()` returning `null` is dangerously ambiguous. It
  /// means *either* "there's no conversation on this page" — perfectly normal — *or*
  /// "the site redesigned and every one of our selectors is now wrong". Those look
  /// identical to the user: Drifterr simply reports no drift, forever, and the natural
  /// conclusion is that detection is broken or useless rather than blind.
  ///
  /// Since these scrapers depend on DOM internals of sites we don't control, that
  /// second case is not hypothetical — it is the expected steady state after any
  /// redesign. A broken scraper must therefore say so.
  ///
  /// The distinguishing signal is the composer: if the page has a chat input *and*
  /// substantial visible text, it is a conversation we're failing to read. If there's
  /// no composer, we're simply not on a chat page.
  ///
  /// Returns `{ ok, reason, host, turns, variant }` where `reason` is one of:
  ///   `ok` | `unsupported_host` | `not_a_chat_page` | `no_conversation_yet`
  ///   | `selectors_stale`
  function diagnose(opts) {
    opts = opts || {};
    const doc = opts.doc || (typeof document !== "undefined" ? document : null);
    const hostname =
      opts.hostname || (typeof location !== "undefined" ? location.hostname : "");
    if (!doc) return { ok: false, reason: "not_a_chat_page", host: hostname, turns: 0 };

    const cfg = pickConfig(hostname);
    if (!cfg) return { ok: false, reason: "unsupported_host", host: hostname, turns: 0 };

    for (let i = 0; i < cfg.variants.length; i++) {
      const turns = turnsForVariant(doc, cfg.variants[i]);
      if (turns.length) {
        return { ok: true, reason: "ok", host: hostname, turns: turns.length, variant: i };
      }
    }

    // Nothing matched. Work out whether that's expected or a breakage.
    let composer = null;
    try {
      composer = cfg.composer ? doc.querySelector(cfg.composer) : null;
    } catch (_e) {
      composer = null;
    }
    const bodyText = ((doc.body && doc.body.innerText) || "").trim();
    if (!composer) {
      // No chat input: a settings page, a 404, a logged-out screen.
      return { ok: false, reason: "not_a_chat_page", host: hostname, turns: 0 };
    }
    // A composer with very little text is a fresh, empty chat — nothing wrong.
    if (bodyText.length < STALE_TEXT_THRESHOLD) {
      return { ok: false, reason: "no_conversation_yet", host: hostname, turns: 0 };
    }
    // A composer plus a page full of text that none of our selectors can see means the
    // selectors are wrong, not the page.
    return { ok: false, reason: "selectors_stale", host: hostname, turns: 0 };
  }

  /// Visible characters above which a chat page is assumed to hold a conversation.
  /// Generous, so a mostly-empty page with placeholder copy is never called broken —
  /// a false "we're broken" is nearly as damaging as silent blindness.
  const STALE_TEXT_THRESHOLD = 400;

  /// A human-readable explanation for a diagnosis, for the popup.
  function explain(d) {
    switch (d && d.reason) {
      case "ok":
        return "Reading this conversation (" + d.turns + " turns).";
      case "unsupported_host":
        return "Drifterr doesn't watch this site.";
      case "not_a_chat_page":
        return "No chat on this page yet.";
      case "no_conversation_yet":
        return "Chat is open — send a message and Drifterr will start watching.";
      case "selectors_stale":
        return (
          "Drifterr can't read this page. " +
          (d.host || "The site") +
          " likely changed its layout, so drift is NOT being tracked here. " +
          "Please report it — the fix is a selector update."
        );
      default:
        return "Unknown state.";
    }
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

  const api = { extract, inject, diagnose, explain };
  if (typeof window !== "undefined") window.DrifterrParse = api;
  if (typeof globalThis !== "undefined") globalThis.DrifterrParse = api;
})();
