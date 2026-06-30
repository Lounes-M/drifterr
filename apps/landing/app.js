// Drifterr landing behaviour: scroll reveal, cursor glow + parallax, the
// animated drift score, FAQ accordion and the pricing toggle. All download
// buttons link to the dedicated /download page, which handles OS detection and
// the actual installers.

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
  setupReveal(document);
  setupParallax(document);
  setupDriftScore(document);
  setupCardSpot(document);
  setupHow(document);
  setupFaq(document);
  setupPricing(document);
}
