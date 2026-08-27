// The sign-in page.
//
// Extracted from an inline <script type="module"> in login.html so the site can
// serve `script-src 'self'` with no `'unsafe-inline'`. See account.js for why.

import { supabase, configured, signInWithProvider, qp } from "./supabase.js";

const msg = document.getElementById("msg");
const show = (text, kind) => { msg.hidden = false; msg.className = "auth-msg " + kind; msg.textContent = text; };

const plan = qp("plan"), interval = qp("interval") || "year";
if (plan) {
  localStorage.setItem("drifterr_pending_checkout", JSON.stringify({ plan, interval }));
  const su = document.getElementById("signup-link");
  if (su) su.href = `/signup?plan=${plan}&interval=${interval}`;
}

if (!configured) show("Accounts aren't enabled yet — you can still download Drifterr for free.", "info");

document.getElementById("form").addEventListener("submit", async (e) => {
  e.preventDefault();
  if (!configured) { window.location.href = "/download"; return; }
  const btn = document.getElementById("submit");
  btn.disabled = true; btn.textContent = "Signing in…";
  try {
    const { error } = await supabase.auth.signInWithPassword({
      email: document.getElementById("email").value.trim(),
      password: document.getElementById("password").value,
    });
    if (error) throw error;
    window.location.href = "/account";
  } catch (err) {
    show(err.message || "Could not sign in.", "error");
    btn.disabled = false; btn.textContent = "Sign in";
  }
});

document.querySelectorAll(".oauth button").forEach((b) =>
  b.addEventListener("click", () => {
    if (!configured) { window.location.href = "/download"; return; }
    signInWithProvider(b.dataset.provider, "/account").catch((e) => show(e.message, "error"));
  })
);
