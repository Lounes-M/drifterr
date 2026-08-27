// The account page: plan, billing portal, sign-out.
//
// Extracted from an inline <script type="module"> in account.html so the site can
// serve `script-src 'self'` with no `'unsafe-inline'`. An inline script is
// indistinguishable, to the browser, from one an injection put there — which is
// the whole reason a strict script-src is worth having on the page that holds a
// signed-in session.

import { configured, currentUser, fetchMe, openPortal, startCheckout, signOut } from "./supabase.js";

const msg = document.getElementById("msg");
const show = (text, kind) => { msg.hidden = false; msg.className = "auth-msg " + kind; msg.textContent = text; };
const fmtDate = (iso) => iso ? new Date(iso).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" }) : "—";

document.getElementById("signout").addEventListener("click", async (e) => {
  e.preventDefault(); await signOut(); window.location.href = "/";
});

async function init() {
  if (!configured) { show("Accounts aren't enabled yet.", "info"); document.getElementById("hello").textContent = ""; return; }

  const user = await currentUser();
  if (!user) { window.location.href = "/login"; return; }

  const params = new URLSearchParams(window.location.search);
  if (params.get("checkout") === "success") show("You're all set — your plan is active. 🎉", "ok");
  if (params.get("checkout") === "cancelled") show("Checkout cancelled — no charge was made.", "info");

  const pending = localStorage.getItem("drifterr_pending_checkout");
  if (pending && !params.get("checkout")) {
    localStorage.removeItem("drifterr_pending_checkout");
    try { const { plan, interval } = JSON.parse(pending); await startCheckout(plan, interval); return; } catch { /* fall through */ }
  }

  try {
    const me = await fetchMe();
    const ent = me.entitlement || {};
    document.getElementById("hello").textContent = `Signed in as ${me.user.email}`;
    document.getElementById("email").textContent = me.user.email || "—";
    document.getElementById("since").textContent = fmtDate(me.profile?.created_at);

    const planId = ent.plan_id || "free";
    const pill = document.getElementById("plan-pill");
    pill.textContent = ent.plan_name || "Free";
    pill.className = "plan-pill" + (planId === "free" ? " free" : "");
    document.getElementById("status").textContent = ent.status || "free";
    if (ent.current_period_end) {
      document.getElementById("renew-row").hidden = false;
      document.getElementById("renew").textContent =
        (ent.cancel_at_period_end ? "Ends " : "Renews ") + fmtDate(ent.current_period_end);
    }
    if (planId !== "free") {
      const manage = document.getElementById("manage");
      manage.hidden = false;
      manage.addEventListener("click", (e) => { e.preventDefault(); openPortal().catch((err) => show(err.message, "error")); });
      document.getElementById("upgrade").innerHTML = 'Change plan <span class="arrow">→</span>';
    }
    document.getElementById("plan-card").hidden = false;
    document.getElementById("account-card").hidden = false;
  } catch (err) {
    show(err.message || "Could not load your account.", "error");
  }
}

init();
