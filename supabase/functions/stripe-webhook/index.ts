// POST /functions/v1/stripe-webhook
//
// Stripe → us. This is the ONLY place a paid plan is granted. We verify the
// signature, then mirror the subscription's state into public.subscriptions
// using the service role. Deploy with `verify_jwt = false` (Stripe doesn't send
// a Supabase JWT) — see supabase/config.toml. Signature verification is what
// authenticates the request instead.
//
// # Delivery is at-least-once and unordered
//
// Stripe retries on any non-2xx and on a timeout, and makes no ordering promise.
// Handling that is not defensive programming here; both failures land directly on
// a paying customer:
//
//   * A retry re-ran the whole sync. Harmless for a plain upsert, but there was
//     no record that an event had been seen, so nothing could be audited after a
//     billing dispute and any future conditional logic would double-apply.
//   * Out-of-order `customer.subscription.updated` events meant last-write-wins
//     on a stale payload: a customer who upgraded could be put back on the plan
//     they upgraded *from*, with nothing in the system explaining why.
//
// So: record the event id first (the primary key is the idempotency guarantee),
// and refuse to apply anything older than what the row already reflects.

import Stripe from "npm:stripe@17.5.0";
import { adminClient, stripe } from "../_shared/clients.ts";

const WEBHOOK_SECRET = Deno.env.get("STRIPE_WEBHOOK_SECRET")!;

// Map a Stripe Price id back to our plan id, using the catalog in the database.
//
// Two `.eq()` filters rather than an interpolated `.or()` string: the price id
// comes from a signature-verified payload so it is not attacker-controlled today,
// but building a filter expression by string concatenation is the wrong shape to
// leave in a billing path, and PostgREST's `or` syntax has its own escaping rules
// that nothing here was applying.
async function priceToPlan(priceId: string | null | undefined): Promise<string | null> {
  if (!priceId) return null;
  const admin = adminClient();
  for (const column of ["stripe_price_monthly", "stripe_price_yearly"]) {
    const { data } = await admin.from("plans").select("id").eq(column, priceId).maybeSingle();
    if (data?.id) return data.id;
  }
  return null;
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

// Apply a subscription's state, unless the row already reflects something newer.
//
// `eventAt` is Stripe's own `created` for the event that carried this payload —
// not our clock, and not the subscription's timestamps, because what we are
// ordering is the events themselves.
async function syncSubscription(sub: Stripe.Subscription, eventAt: number) {
  const userId = await resolveUserId(sub);
  if (!userId) {
    console.error("webhook: no user for subscription", sub.id);
    return;
  }

  const admin = adminClient();

  // Ordering guard. A row we have never seen has last_event_at = 0, so a first
  // event always applies.
  const { data: existing } = await admin
    .from("subscriptions")
    .select("last_event_at")
    .eq("user_id", userId)
    .maybeSingle();
  if (existing && Number(existing.last_event_at ?? 0) > eventAt) {
    console.log(
      `webhook: skipping stale event for ${userId} (event ${eventAt} < applied ${existing.last_event_at})`,
    );
    return;
  }

  const item = sub.items.data[0];
  const priceId = item?.price?.id ?? null;
  const plan = (await priceToPlan(priceId)) ?? "free";
  const canceled = sub.status === "canceled" || sub.status === "incomplete_expired";

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
      last_event_at: eventAt,
    },
    { onConflict: "user_id" },
  );
}

// A failed payment does not cancel a subscription — Stripe keeps retrying, and
// the customer keeps their plan through the dunning window. But `past_due` is
// what lets the account page say "your card was declined" instead of leaving
// someone to discover it when the plan silently disappears weeks later.
async function markPastDue(invoice: Stripe.Invoice, eventAt: number) {
  const subId = typeof invoice.subscription === "string"
    ? invoice.subscription
    : invoice.subscription?.id;
  if (!subId) return;
  const sub = await stripe.subscriptions.retrieve(subId);
  await syncSubscription(sub, eventAt);
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

  // Claim the event before doing any work.
  //
  // The insert is the lock: `stripe_events.id` is the primary key, so a
  // redelivery — or two deliveries racing in separate function instances —
  // loses here rather than applying the change twice. Claiming *first* is
  // deliberate: claiming afterwards would leave a crash mid-handler looking
  // like a completed event, and a subscription change dropped on the floor is
  // worse than one applied twice.
  const admin = adminClient();
  const { error: claimErr } = await admin.from("stripe_events").insert({
    id: event.id,
    type: event.type,
    event_at: event.created,
  });
  if (claimErr) {
    // 23505 = unique_violation: we have already handled this event. Answer 200 so
    // Stripe stops retrying.
    if (claimErr.code === "23505") {
      return new Response(JSON.stringify({ received: true, duplicate: true }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    }
    // Anything else means we could not record the claim, so we must not act on
    // the event either — 500 asks Stripe to send it again.
    console.error("webhook: could not claim event", claimErr);
    return new Response("could not record event", { status: 500 });
  }

  try {
    switch (event.type) {
      case "customer.subscription.created":
      case "customer.subscription.updated":
      case "customer.subscription.deleted":
        await syncSubscription(event.data.object as Stripe.Subscription, event.created);
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
          await syncSubscription(sub, event.created);
        }
        break;
      }

      case "invoice.payment_failed":
        await markPastDue(event.data.object as Stripe.Invoice, event.created);
        break;

      default:
        // Ignore the rest — we only care about subscription lifecycle.
        break;
    }
  } catch (e) {
    console.error("webhook handler error", e);
    // Release the claim so Stripe's retry can actually retry. Leaving it would
    // turn one transient failure into a permanently dropped subscription change.
    await admin.from("stripe_events").delete().eq("id", event.id);
    return new Response("handler error", { status: 500 });
  }

  return new Response(JSON.stringify({ received: true }), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
});
