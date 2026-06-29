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
  const items = doc.querySelectorAll(".reveal");
  if (!("IntersectionObserver" in window)) {
    items.forEach((el) => el.classList.add("in"));
    return;
  }
  const io = new IntersectionObserver(
    (entries) =>
      entries.forEach((e) => {
        if (e.isIntersecting) {
          e.target.classList.add("in");
          io.unobserve(e.target);
        }
      }),
    { threshold: 0.14 }
  );
  items.forEach((el) => io.observe(el));
}

// Pointer-reactive glow on feature cards (the spotlight follows the cursor).
function setupCardGlow(doc) {
  doc.querySelectorAll(".card").forEach((card) => {
    card.addEventListener("pointermove", (e) => {
      const r = card.getBoundingClientRect();
      card.style.setProperty("--mx", `${e.clientX - r.left}px`);
      card.style.setProperty("--my", `${e.clientY - r.top}px`);
    });
  });
}

// Gentle parallax tilt on the hero product window, tied to the cursor.
function setupTilt(doc) {
  const tilt = doc.getElementById("tilt");
  const win = tilt && tilt.querySelector(".window");
  if (!win || window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  if (window.matchMedia("(max-width: 860px)").matches) return;
  window.addEventListener("pointermove", (e) => {
    const x = (e.clientX / window.innerWidth - 0.5) * 2;
    const y = (e.clientY / window.innerHeight - 0.5) * 2;
    win.style.transform = `perspective(900px) rotateY(${x * 4}deg) rotateX(${-y * 4}deg)`;
  });
}

if (typeof document !== "undefined" && typeof navigator !== "undefined") {
  const os = detectOS(navigator.userAgent, navigator.platform);
  apply(document, os);
  setupReveal(document);
  setupCardGlow(document);
  setupTilt(document);
}
