// Adaptive download button: detect the visitor's OS and label the button for it.
// Links stay on our own domain (/download/<os>); a serverless redirect
// (api/download.js) resolves the latest installer and streams it straight to the
// visitor — they never land on the GitHub repo.

const DOWNLOAD = (os) => `/download/${os}`;

export function detectOS(ua, platform) {
  const s = `${platform || ""} ${ua || ""}`.toLowerCase();
  if (/mac|iphone|ipad|ios/.test(s)) return "mac";
  if (/win/.test(s)) return "win";
  if (/linux|x11|android/.test(s)) return "linux";
  return "other";
}

const LABELS = {
  mac: "Download for macOS",
  win: "Download for Windows",
  linux: "Download for Linux",
  other: "Download",
};

const SUBS = {
  mac: "macOS 11+ · Apple silicon & Intel — free",
  win: "Windows 10+ — free",
  linux: "AppImage / .deb — free",
  other: "macOS · Windows · Linux — free",
};

export function apply(doc, os) {
  const label = LABELS[os] || LABELS.other;
  // "other" (unknown OS) gets a chooser; everyone else a direct OS download.
  const href = os === "other" ? DOWNLOAD("") : DOWNLOAD(os);
  for (const id of ["download", "download-2", "nav-download"]) {
    const el = doc.getElementById(id);
    if (el) {
      el.textContent = id === "nav-download" ? "Download" : label;
      el.href = href;
    }
  }
  for (const id of ["cta-sub", "cta-sub-2"]) {
    const el = doc.getElementById(id);
    if (el) el.textContent = SUBS[os] || SUBS.other;
  }
  return label;
}

// Reveal-on-scroll for a modern feel (no-op if IntersectionObserver missing).
function setupReveal(doc) {
  const items = doc.querySelectorAll(".glass, .card, .step, .glass-img");
  if (!("IntersectionObserver" in window)) {
    items.forEach((el) => el.classList.add("in"));
    return;
  }
  const io = new IntersectionObserver(
    (entries) => entries.forEach((e) => e.isIntersecting && e.target.classList.add("in")),
    { threshold: 0.12 }
  );
  items.forEach((el) => io.observe(el));
}

if (typeof document !== "undefined" && typeof navigator !== "undefined") {
  const os = detectOS(navigator.userAgent, navigator.platform);
  apply(document, os);
  setupReveal(document);
}
