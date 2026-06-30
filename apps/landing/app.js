// Drifterr landing behaviour: adaptive download, scroll reveal, cursor glow +
// parallax, the animated drift score, FAQ accordion and the pricing toggle.
// Downloads stay on our own domain (/download/<os>) — a serverless redirect
// (api/download.js) resolves the latest installer.

const DOWNLOAD = (os) => `/download/${os}`;

export function detectOS(ua, platform) {
  const s = `${platform || ""} ${ua || ""}`.toLowerCase();
  if (/mac|iphone|ipad|ios/.test(s)) return "mac";
  if (/win/.test(s)) return "win";
  if (/linux|x11|android/.test(s)) return "linux";
  return "other";
}

const LABELS = { mac: "Download for macOS", win: "Download for Windows", linux: "Download for Linux", other: "Download" };
const SUBS = {
  mac: "macOS 11+ · Apple silicon & Intel — free",
  win: "Windows 10+ — free",
  linux: "AppImage / .deb — free",
  other: "macOS · Windows · Linux — free",
};

export function apply(doc, os) {
  const label = LABELS[os] || LABELS.other;
  const href = os === "other" ? DOWNLOAD("") : DOWNLOAD(os);
  // Primary CTAs carry the adaptive label + arrow; secondary ones stay short.
  for (const id of ["download", "download-3"]) {
    const el = doc.getElementById(id);
    if (el) { el.innerHTML = `${label} <span class="arrow">→</span>`; el.href = href; }
  }
  for (const id of ["download-2", "nav-download"]) {
    const el = doc.getElementById(id);
    if (el) { el.textContent = "Download"; el.href = href; }
  }
  for (const id of ["cta-sub"]) {
    const el = doc.getElementById(id);
    if (el) el.textContent = SUBS[os] || SUBS.other;
  }
  return label;
}

function setupReveal(doc) {
  const items = doc.querySelectorAll(".reveal");
  if (!("IntersectionObserver" in window)) { items.forEach((el) => el.classList.add("in")); return; }
  const io = new IntersectionObserver(
    (entries) => entries.forEach((e) => { if (e.isIntersecting) { e.target.classList.add("in"); io.unobserve(e.target); } }),
    { threshold: 0.14 }
  );
  items.forEach((el) => io.observe(el));
}

// Cursor glow + depth-based parallax on the background blobs.
function setupParallax(doc) {
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  const glow = doc.getElementById("cursor-glow");
  const layers = [...doc.querySelectorAll("[data-depth]")];
  const target = { x: 0, y: 0 }, curr = { x: 0, y: 0 };
  window.addEventListener("pointermove", (e) => {
    target.x = e.clientX / window.innerWidth - 0.5;
    target.y = e.clientY / window.innerHeight - 0.5;
    if (glow) { glow.style.left = e.clientX + "px"; glow.style.top = e.clientY + "px"; glow.style.opacity = "1"; }
  }, { passive: true });
  (function tick() {
    curr.x += (target.x - curr.x) * 0.06;
    curr.y += (target.y - curr.y) * 0.06;
    for (const el of layers) {
      const d = parseFloat(el.getAttribute("data-depth")) || 0;
      el.style.transform = `translate3d(${(curr.x * d).toFixed(2)}px,${(curr.y * d).toFixed(2)}px,0)`;
    }
    requestAnimationFrame(tick);
  })();
}

// Count the drift score up to 87% once the mockup scrolls into view.
function setupDriftScore(doc) {
  const el = doc.getElementById("driftScore");
  if (!el) return;
  let done = false;
  const run = () => {
    if (done) return; done = true;
    const dur = 1700, start = performance.now(), to = 87;
    (function step(now) {
      const t = Math.min(1, (now - start) / dur);
      el.textContent = Math.round((1 - Math.pow(1 - t, 3)) * to);
      if (t < 1) requestAnimationFrame(step);
    })(start);
  };
  if (!("IntersectionObserver" in window)) return run();
  const io = new IntersectionObserver((es) => es.forEach((e) => { if (e.isIntersecting) { run(); io.disconnect(); } }), { threshold: 0.4 });
  io.observe(el);
}

// Cursor spotlight on the bento feature cards (drives the --mx/--my glow).
function setupCardSpot(doc) {
  doc.querySelectorAll(".card[data-spot]").forEach((card) => {
    card.addEventListener("pointermove", (e) => {
      const r = card.getBoundingClientRect();
      card.style.setProperty("--mx", `${e.clientX - r.left}px`);
      card.style.setProperty("--my", `${e.clientY - r.top}px`);
    });
  });
}

// Download clicks: resolve whether a release exists. If it does, go through the
// on-domain redirect (/download/<os>) which streams the installer; if not, show
// a clear on-page notice instead of navigating to a dead "coming soon" page.
const REPO = "Lounes-M/drifterr";
let _releaseProbe;
async function hasRelease() {
  if (_releaseProbe !== undefined) return _releaseProbe;
  try {
    const r = await fetch(`https://api.github.com/repos/${REPO}/releases/latest`, { headers: { Accept: "application/vnd.github+json" } });
    if (!r.ok) return (_releaseProbe = false);
    const data = await r.json();
    _releaseProbe = Array.isArray(data.assets) && data.assets.length > 0;
  } catch { _releaseProbe = false; }
  return _releaseProbe;
}

function toast(msg) {
  let t = document.getElementById("toast");
  if (!t) { t = document.createElement("div"); t.id = "toast"; t.className = "toast"; document.body.appendChild(t); }
  t.textContent = msg;
  requestAnimationFrame(() => t.classList.add("show"));
  clearTimeout(t._h); t._h = setTimeout(() => t.classList.remove("show"), 4600);
}

// First-launch help (the builds are unsigned, so the OS warns once).
const FIRST_LAUNCH = {
  mac: "Downloading… First launch: right-click the app → Open (it's unsigned).",
  win: "Downloading… If SmartScreen appears: More info → Run anyway.",
  linux: "Downloading… Make it executable (chmod +x) and run the AppImage.",
  other: "Downloading…",
};

function setupDownloadClicks(doc) {
  doc.querySelectorAll('a[href^="/download"]').forEach((a) => {
    a.addEventListener("click", async (e) => {
      e.preventDefault();
      const os = detectOS(navigator.userAgent, navigator.platform);
      const ok = await hasRelease();
      if (ok) {
        // A 302-to-file download keeps the page, so the tip stays visible.
        window.location.href = os === "other" ? "/download" : `/download/${os}`;
        toast(FIRST_LAUNCH[os] || FIRST_LAUNCH.other);
      } else {
        toast("The installer isn't out yet — the first build lands here very soon. ⏳");
      }
    });
  });
}

// How-it-works: a tabbed live demo. Click a step to switch; while the section
// is hovered it auto-advances through the steps (calm at rest, lively on focus).
function setupHow(doc) {
  const grid = doc.querySelector(".how-grid");
  if (!grid) return;
  const steps = [...grid.querySelectorAll(".how-step")];
  const states = [...grid.querySelectorAll(".hp-state")];
  let cur = 0, timer = null;
  const show = (n) => {
    cur = n;
    steps.forEach((s, i) => s.classList.toggle("active", i === n));
    states.forEach((s, i) => (s.hidden = i !== n));
  };
  const stop = () => { if (timer) { clearInterval(timer); timer = null; } };
  const start = () => { stop(); if (grid.classList.contains("advancing")) timer = setInterval(() => show((cur + 1) % steps.length), 3800); };
  steps.forEach((s, i) => s.addEventListener("click", () => { show(i); start(); }));
  if (!window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    grid.addEventListener("pointerenter", () => { grid.classList.add("advancing"); start(); });
    grid.addEventListener("pointerleave", () => { grid.classList.remove("advancing"); stop(); });
  }
  show(0);
}

function setupFaq(doc) {
  doc.querySelectorAll(".qa button").forEach((btn) => {
    btn.addEventListener("click", () => btn.parentElement.classList.toggle("open"));
  });
}

function setupPricing(doc) {
  const m = doc.getElementById("bill-monthly"), a = doc.getElementById("bill-annual");
  const pro = doc.getElementById("proPrice"), team = doc.getElementById("teamPrice");
  const proNote = doc.getElementById("proNote"), teamNote = doc.getElementById("teamNote");
  if (!m || !a) return;
  const set = (annual) => {
    a.classList.toggle("on", annual); m.classList.toggle("on", !annual);
    if (pro) pro.textContent = annual ? "9" : "12";
    if (team) team.textContent = annual ? "16" : "20";
    const note = annual ? "billed annually" : "billed monthly";
    if (proNote) proNote.textContent = note;
    if (teamNote) teamNote.textContent = note;
  };
  m.addEventListener("click", () => set(false));
  a.addEventListener("click", () => set(true));
}

if (typeof document !== "undefined" && typeof navigator !== "undefined") {
  apply(document, detectOS(navigator.userAgent, navigator.platform));
  setupReveal(document);
  setupParallax(document);
  setupDriftScore(document);
  setupCardSpot(document);
  setupDownloadClicks(document);
  setupHow(document);
  setupFaq(document);
  setupPricing(document);
}
