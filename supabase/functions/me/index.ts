// GET /functions/v1/me
//
// Auth: Bearer <supabase access token> (required).
// Returns the caller's profile + current entitlement in one round-trip, the
// shape the desktop app and account page render. RLS-respecting (uses the
// caller's JWT), so it can only ever return the caller's own data.

import { preflight, json } from "../_shared/cors.ts";
import { getUser, userClient } from "../_shared/clients.ts";

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

  return json(
    {
      user: { id: user.id, email: user.email },
      profile: profile ?? null,
      entitlement: ent ?? { plan_id: "free", plan_name: "Free", status: "free", features: {} },
    },
    200,
    origin,
  );
});
