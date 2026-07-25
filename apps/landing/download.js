// Drifterr download hub. Detects the visitor's OS for the recommended card,
// runs the actual downloads through the on-domain redirect (/dl/<os>), copies
// CLI commands, and shows an animated progress toast. No external deps.

import * as analytics from "./analytics.js";

const REPO = "Lounes-M/drifterr";

const APPLE = '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M17.05 12.04c-.03-2.6 2.13-3.85 2.22-3.91-1.21-1.77-3.09-2.01-3.76-2.04-1.6-.16-3.12.94-3.93.94-.81 0-2.06-.92-3.39-.89-1.74.03-3.35 1.01-4.25 2.57-1.81 3.14-.46 7.78 1.3 10.33.86 1.25 1.88 2.65 3.22 2.6 1.29-.05 1.78-.83 3.34-.83 1.56 0 2 .83 3.37.81 1.39-.03 2.27-1.27 3.12-2.53.98-1.45 1.39-2.85 1.41-2.92-.03-.01-2.71-1.04-2.74-4.13M14.6 4.6c.71-.86 1.19-2.06 1.06-3.25-1.02.04-2.26.68-3 1.54-.66.76-1.24 1.98-1.08 3.15 1.14.09 2.31-.58 3.02-1.44"/></svg>';
const WIN = '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M3 5.5 10.5 4.4v7.1H3zM10.5 12.5v7.1L3 18.5v-6zM11.6 4.2 21 3v8.5h-9.4zM21 12.5V21l-9.4-1.3v-7.2z"/></svg>';
const LIN = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="m7 9 3 3-3 3M13 15h4"/></svg>';

const OS = {
  mac: { name: "macOS · Universal", meta: "Universal .dmg · Apple silicon & Intel · macOS 12+", label: "Download for macOS", icon: APPLE,
    tip: "First launch: right-click the app → Open (it's unsigned)." },
  win: { name: "Windows 10 / 11", meta: ".exe installer · per-user · no admin", label: "Download for Windows", icon: WIN,
    tip: "If SmartScreen appears: More info → Run anyway." },
  linux: { name: "Linux · AppImage", meta: ".AppImage · x86_64", label: "Download for Linux", icon: LIN,
    tip: "Make it executable (chmod +x) and run the AppImage." },
  deb: { name: "Linux · .deb", meta: "Debian / Ubuntu · x86_64", label: "Download .deb", icon: LIN,
    tip: "Install with: sudo dpkg -i the downloaded .deb" },
};

function detectOS() {
  const s = `${navigator.platform || ""} ${navigator.userAgent || ""}`.toLowerCase();
  // Check mac first: a Mac UA contains "macintosh" even when navigator.platform
  // isn't overridden. Then Windows, then Linux.
  if (/mac|iphone|ipad|ios|darwin/.test(s)) return "mac";
  if (/win/.test(s)) return "win";
  if (/linux|x11|android/.test(s)) return "linux";
  return "mac";
}

let _probe;
async function releaseReady() {
  if (_probe !== undefined) return _probe;
  try {
    const r = await fetch(`https://api.github.com/repos/${REPO}/releases/latest`, { headers: { Accept: "application/vnd.github+json" } });
    if (!r.ok) return (_probe = false);
    const data = await r.json();
    _probe = Array.isArray(data.assets) && data.assets.length > 0;
    if (data.tag_name) showVersion(data.tag_name);
  } catch { _probe = false; }
  return _probe;
}

// Keep the version badge in sync with the actual latest release so it never
// goes stale — the HTML value is just a fallback for offline / API-down.
function showVersion(tag) {
  const ver = tag.startsWith("v") ? tag : `v${tag}`;
  document.querySelectorAll(".ver").forEach((el) => (el.textContent = ver));
}

let toastTimer, toastHide;
function showDownloadToast(osKey) {
  const t = document.getElementById("toast");
  const title = document.getElementById("toast-title");
  const bar = document.getElementById("toast-bar");
  const tip = document.getElementById("toast-tip");
  clearInterval(toastTimer); clearTimeout(toastHide);
  title.textContent = "Starting download…";
  tip.textContent = (OS[osKey] && OS[osKey].tip) || "";
  bar.style.width = "0%";
  t.style.transform = "translate(-50%,0)";
  let p = 0;
  toastTimer = setInterval(() => {
    p += Math.random() * 22 + 10;
    if (p >= 100) { p = 100; clearInterval(toastTimer); title.textContent = "Download ready ✓"; }
    bar.style.width = p + "%";
  }, 300);
  toastHide = setTimeout(() => { t.style.transform = "translate(-50%,160%)"; }, 6000);
}
function showMsgToast(msg) {
  const t = document.getElementById("toast");
  document.getElementById("toast-title").textContent = msg;
  document.getElementById("toast-tip").textContent = "";
  document.getElementById("toast-bar").style.width = "0%";
  clearInterval(toastTimer); clearTimeout(toastHide);
  t.style.transform = "translate(-50%,0)";
  toastHide = setTimeout(() => { t.style.transform = "translate(-50%,160%)"; }, 4200);
}

async function triggerDownload(osKey) {
  analytics.track("download_click", { os: osKey });
  const ok = await releaseReady();
  if (!ok) {
    // Worth measuring on its own: "wanted it, couldn't have it" is a different
    // funnel loss from "never clicked".
    analytics.track("download_failed", { os: osKey });
    showMsgToast("The installer isn't out yet — landing here very soon. ⏳");
    return;
  }
  analytics.track("download_started", { os: osKey });
  // hidden iframe keeps the page (and the toast) put while the file downloads
  let f = document.getElementById("dl-frame");
  if (!f) { f = document.createElement("iframe"); f.id = "dl-frame"; f.style.display = "none"; document.body.appendChild(f); }
  f.src = `/dl/${osKey}`;
  showDownloadToast(osKey);
}

function setupReveal() {
  const items = document.querySelectorAll(".reveal");
  if (!("IntersectionObserver" in window)) { items.forEach((el) => el.classList.add("in")); return; }
  const io = new IntersectionObserver((es) => es.forEach((e) => { if (e.isIntersecting) { e.target.classList.add("in"); io.unobserve(e.target); } }), { threshold: 0.12 });
  items.forEach((el) => io.observe(el));
}

function setupParallax() {
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  const glow = document.getElementById("cursor-glow");
  const layers = [...document.querySelectorAll("[data-depth]")];
  const target = { x: 0, y: 0 }, curr = { x: 0, y: 0 };
  window.addEventListener("pointermove", (e) => {
    target.x = e.clientX / window.innerWidth - 0.5;
    target.y = e.clientY / window.innerHeight - 0.5;
    if (glow) { glow.style.left = e.clientX + "px"; glow.style.top = e.clientY + "px"; glow.style.opacity = "1"; }
    const tile = e.target.closest("[data-spot]");
    if (tile) { const r = tile.getBoundingClientRect(); tile.style.setProperty("--mx", `${e.clientX - r.left}px`); tile.style.setProperty("--my", `${e.clientY - r.top}px`); }
  }, { passive: true });
  (function tick() {
    curr.x += (target.x - curr.x) * 0.06; curr.y += (target.y - curr.y) * 0.06;
    for (const el of layers) { const d = parseFloat(el.getAttribute("data-depth")) || 0; el.style.transform = `translate3d(${(curr.x * d).toFixed(2)}px,${(curr.y * d).toFixed(2)}px,0)`; }
    requestAnimationFrame(tick);
  })();
}

function init() {
  // refresh the version badge from the latest release (fire-and-forget)
  releaseReady();

  // recommended card
  const os = detectOS();
  const cfg = OS[os];
  document.getElementById("rec-ic").innerHTML = cfg.icon;
  document.getElementById("rec-name").textContent = cfg.name;
  document.getElementById("rec-meta").textContent = cfg.meta;
  document.getElementById("rec-label").textContent = cfg.label;
  document.getElementById("rec-btn").addEventListener("click", () => triggerDownload(os));

  // First-launch notice: promote the visitor's own platform to the front, so the
  // steps that apply to them are the first thing they read. The `.deb` tile maps
  // onto the same Linux block.
  const noticeOs = os === "deb" ? "linux" : os;
  const mine = document.querySelector(`[data-os-block="${noticeOs}"]`);
  if (mine) mine.classList.add("mine");

  // Funnel: page view + whether the unsigned-build help was actually seen.
  analytics.init(document);

  // platform tiles
  document.querySelectorAll(".dl[data-os]").forEach((b) => b.addEventListener("click", () => triggerDownload(b.dataset.os)));

  // browser extensions — not on stores yet
  document.querySelectorAll("[data-soon]").forEach((el) => el.addEventListener("click", () => showMsgToast(`${el.dataset.soon} extension — coming soon. ⏳`)));

  // CLI copy
  document.querySelectorAll(".copy[data-cmd]").forEach((b) => b.addEventListener("click", async () => {
    try { await navigator.clipboard.writeText(b.dataset.cmd); } catch {}
    const old = b.textContent; b.textContent = "Copied ✓";
    setTimeout(() => (b.textContent = old), 1500);
  }));

  // see-all scroll
  const see = document.getElementById("see-all");
  if (see) see.addEventListener("click", () => document.getElementById("all").scrollIntoView({ behavior: "smooth" }));

  setupReveal();
  setupParallax();
}

if (typeof document !== "undefined") init();
