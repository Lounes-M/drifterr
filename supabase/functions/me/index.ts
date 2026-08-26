// GET /functions/v1/me
//
// Auth: Bearer <supabase access token> (required).
// Returns the caller's profile + current entitlement in one round-trip, the
// shape the desktop app and account page render. RLS-respecting (uses the
// caller's JWT), so it can only ever return the caller's own data.

import { preflight, json } from "../_shared/cors.ts";
import { getUser, userClient } from "../_shared/clients.ts";
import { signPlanToken } from "../_shared/plan_token.ts";

Deno.serve(async (req) => {
  const pre = preflight(req);
  if (pre) return pre;
  const origin = req.headers.get("origin");

  const user = await getUser(req);
  if (!user) return json({ error: "unauthorized" }, 401, origin);

  const supa = userClient(req);
  const [{ data: profile }, { data: ent }] = await Promise.all([
    supa.from("profiles").select("email, full_name, stripe_customer_id, created_at").eq("id", user.id).single(),
    supa.from("my_entitlement").select("*").maybeSingle(),
  ]);

  const entitlement = ent ??
    { plan_id: "free", plan_name: "Free", status: "free", features: {} };

  // A signed assertion of the plan, for the desktop app.
  //
  // The app used to read `plan_id` here and simply tell its local proxy what it
  // was; the proxy stored whatever it was told. Signing the claim means the proxy
  // verifies a plan rather than trusting one. Short-lived, so a cancellation stops
  // mattering within a day, but long enough that a flight or a Supabase outage does
  // not strip a paying customer of the features they bought.
  //
  // Absent when no signing key is configured — the proxy then reports the
  // entitlement as unverified rather than pretending otherwise.
  const planToken = await signPlanToken(user.id, String(entitlement.plan_id ?? "free"));

  return json(
    {
      user: { id: user.id, email: user.email },
      profile: profile ?? null,
      entitlement,
      ...(planToken ? { planToken } : {}),
    },
    200,
    origin,
  );
});
