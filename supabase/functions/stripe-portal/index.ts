// POST /functions/v1/stripe-portal
//
// Auth: Bearer <supabase access token> (required).
// Returns a Stripe Billing Portal URL so the user can manage / cancel their
// subscription and update payment details. No body needed.

import { json, preflight } from "../_shared/cors.ts";
import { adminClient, getUser, stripe } from "../_shared/clients.ts";

const SITE_URL = Deno.env.get("SITE_URL") ?? "https://drifterr.app";

Deno.serve(async (req) => {
  const pre = preflight(req);
  if (pre) return pre;
  const origin = req.headers.get("origin");

  if (req.method !== "POST") {
    return json({ error: "method not allowed" }, 405, origin);
  }

  const user = await getUser(req);
  if (!user) return json({ error: "unauthorized" }, 401, origin);

  const admin = adminClient();
  const { data: profile } = await admin
    .from("profiles")
    .select("stripe_customer_id")
    .eq("id", user.id)
    .single();

  if (!profile?.stripe_customer_id) {
    return json({ error: "no billing account yet" }, 400, origin);
  }

  try {
    const session = await stripe.billingPortal.sessions.create({
      customer: profile.stripe_customer_id,
      return_url: `${SITE_URL}/account`,
    });
    return json({ url: session.url }, 200, origin);
  } catch (e) {
    console.error("portal error", e);
    return json({ error: "could not open billing portal" }, 500, origin);
  }
});
