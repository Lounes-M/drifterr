// POST /functions/v1/stripe-webhook
//
// Stripe → us. This is the ONLY place a paid plan is granted. We verify the
// signature, then mirror the subscription's state into public.subscriptions
// using the service role. Deploy with `verify_jwt = false` (Stripe doesn't send
// a Supabase JWT) — see supabase/config.toml. Signature verification is what
// authenticates the request instead.

import Stripe from "https://esm.sh/stripe@17.5.0?target=deno";
import { adminClient, stripe } from "../_shared/clients.ts";

const WEBHOOK_SECRET = Deno.env.get("STRIPE_WEBHOOK_SECRET")!;

// Map a Stripe Price id back to our plan id. Built once from env at boot.
function priceToPlan(priceId: string | null | undefined): string | null {
  if (!priceId) return null;
  const table: Record<string, string> = {
    [Deno.env.get("STRIPE_PRICE_PRO_MONTHLY") ?? "_"]: "pro",
    [Deno.env.get("STRIPE_PRICE_PRO_YEARLY") ?? "_"]: "pro",
    [Deno.env.get("STRIPE_PRICE_TEAM_MONTHLY") ?? "_"]: "team",
    [Deno.env.get("STRIPE_PRICE_TEAM_YEARLY") ?? "_"]: "team",
  };
  return table[priceId] ?? null;
}

// Pull the supabase user id off the subscription/customer metadata.
async function resolveUserId(sub: Stripe.Subscription): Promise<string | null> {
  const fromSub = sub.metadata?.supabase_user_id;
  if (fromSub) return fromSub;
  const admin = adminClient();
  const customerId = typeof sub.customer === "string" ? sub.customer : sub.customer?.id;
  if (!customerId) return null;
  const { data } = await admin
    .from("profiles")
    .select("id")
    .eq("stripe_customer_id", customerId)
    .single();
  return data?.id ?? null;
}

async function syncSubscription(sub: Stripe.Subscription) {
  const userId = await resolveUserId(sub);
  if (!userId) {
    console.error("webhook: no user for subscription", sub.id);
    return;
  }

  const item = sub.items.data[0];
  const priceId = item?.price?.id ?? null;
  const plan = priceToPlan(priceId) ?? "free";
  const canceled = sub.status === "canceled" || sub.status === "incomplete_expired";

  const admin = adminClient();
  await admin.from("subscriptions").upsert(
    {
      user_id: userId,
      plan_id: canceled ? "free" : plan,
      status: canceled ? "free" : sub.status,
      stripe_subscription_id: sub.id,
      stripe_price_id: priceId,
      interval: item?.price?.recurring?.interval ?? null,
      seats: item?.quantity ?? 1,
      cancel_at_period_end: sub.cancel_at_period_end ?? false,
      current_period_end: sub.current_period_end
        ? new Date(sub.current_period_end * 1000).toISOString()
        : null,
    },
    { onConflict: "user_id" },
  );
}

Deno.serve(async (req) => {
  if (req.method !== "POST") return new Response("method not allowed", { status: 405 });

  const signature = req.headers.get("stripe-signature");
  if (!signature) return new Response("missing signature", { status: 400 });

  const raw = await req.text();
  let event: Stripe.Event;
  try {
    event = await stripe.webhooks.constructEventAsync(raw, signature, WEBHOOK_SECRET);
  } catch (e) {
    console.error("webhook signature verification failed", e);
    return new Response("bad signature", { status: 400 });
  }

  try {
    switch (event.type) {
      case "customer.subscription.created":
      case "customer.subscription.updated":
      case "customer.subscription.deleted":
        await syncSubscription(event.data.object as Stripe.Subscription);
        break;

      case "checkout.session.completed": {
        // The subscription object arrives in its own event too, but fetch it
        // here so the upgrade is reflected the instant checkout returns.
        const session = event.data.object as Stripe.Checkout.Session;
        if (session.subscription) {
          const subId = typeof session.subscription === "string"
            ? session.subscription
            : session.subscription.id;
          const sub = await stripe.subscriptions.retrieve(subId);
          await syncSubscription(sub);
        }
        break;
      }

      default:
        // Ignore the rest — we only care about subscription lifecycle.
        break;
    }
  } catch (e) {
    console.error("webhook handler error", e);
    return new Response("handler error", { status: 500 });
  }

  return new Response(JSON.stringify({ received: true }), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
});
