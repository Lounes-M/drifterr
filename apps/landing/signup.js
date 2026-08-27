// The sign-up page.
//
// Extracted from an inline <script type="module"> in signup.html so the site can
// serve `script-src 'self'` with no `'unsafe-inline'`. See account.js for why.

import { supabase, configured, signInWithProvider, qp } from "./supabase.js";

const msg = document.getElementById("msg");
const show = (text, kind) => { msg.hidden = false; msg.className = "auth-msg " + kind; msg.textContent = text; };

// Carry the chosen plan through sign-up so checkout resumes after login.
const plan = qp("plan"), interval = qp("interval") || "year";
if (plan) {
  localStorage.setItem("drifterr_pending_checkout", JSON.stringify({ plan, interval }));
  for (const a of ["login-link"]) {
    const el = document.getElementById(a);
    if (el) el.href = `/login?plan=${plan}&interval=${interval}`;
  }
}

if (!configured) show("Accounts aren't enabled yet — you can still download Drifterr for free.", "info");

document.getElementById("form").addEventListener("submit", async (e) => {
  e.preventDefault();
  if (!configured) { window.location.href = "/download"; return; }
  const btn = document.getElementById("submit");
  btn.disabled = true; btn.textContent = "Creating…";
  try {
    const { data, error } = await supabase.auth.signUp({
      email: document.getElementById("email").value.trim(),
      password: document.getElementById("password").value,
      options: {
        data: { full_name: document.getElementById("name").value.trim() },
        emailRedirectTo: `${window.location.origin}/account`,
      },
    });
    if (error) throw error;
    if (data.session) {
      window.location.href = "/account";        // confirmations off → straight in
    } else {
      show("Check your email to confirm your account, then sign in.", "ok");
    }
  } catch (err) {
    show(err.message || "Could not create the account.", "error");
  } finally {
    btn.disabled = false; btn.textContent = "Create account";
  }
});

document.querySelectorAll(".oauth button").forEach((b) =>
  b.addEventListener("click", () => {
    if (!configured) { window.location.href = "/download"; return; }
    signInWithProvider(b.dataset.provider, "/account").catch((e) => show(e.message, "error"));
  })
);
