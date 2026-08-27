// The webhook's decisions, separated from its I/O.
//
// # Why this file exists
//
// The handler used to reach straight for a Supabase client, so its behaviour
// could only be checked by reading it. That is a poor trade anywhere and a bad
// one here: this code decides which plan a paying customer is on, and the two
// bugs it was written to fix — a redelivered event applied twice, and an
// out-of-order event downgrading someone — are precisely the kind that look
// fine on inspection and only appear under Stripe's real delivery behaviour.
//
// So the decisions live here behind a [`Ledger`] interface. The real one talks
// to Postgres; the tests hand in a fake and drive the exact sequences Stripe
// produces. Nothing about the logic changed in the extraction — the point is
// that it is now provable rather than reviewed.

import type Stripe from "npm:stripe@17.5.0";

/** The row we mirror a Stripe subscription into. */
export interface SubscriptionRow {
  user_id: string;
  plan_id: string;
  status: string;
  stripe_subscription_id: string;
  stripe_price_id: string | null;
  interval: string | null;
  seats: number;
  cancel_at_period_end: boolean;
  current_period_end: string | null;
  last_event_at: number;
}

/** Why a claim attempt ended the way it did. */
export type ClaimOutcome = "claimed" | "duplicate" | "error";

/**
 * Everything the handler needs from the outside world.
 *
 * Deliberately small and boring: each method is one question or one write, so a
 * fake is a few lines and there is nowhere for behaviour to hide behind a query
 * builder.
 */
export interface Ledger {
  /** Record that we are handling `id`. `duplicate` means somebody already did. */
  claimEvent(id: string, type: string, eventAt: number): Promise<ClaimOutcome>;
  /** Undo a claim, so Stripe's retry can actually retry. */
  releaseEvent(id: string): Promise<void>;
  /** Stripe `created` of the newest event already applied to this user, or null. */
  lastEventAt(userId: string): Promise<number | null>;
  /** Insert or update the user's subscription row. */
  upsertSubscription(row: SubscriptionRow): Promise<void>;
  /** Our plan id for a Stripe price id, or null when the price is unknown. */
  planForPrice(priceId: string | null): Promise<string | null>;
  /** The Supabase user behind a Stripe customer id, or null. */
  userForCustomer(customerId: string): Promise<string | null>;
  /** Fetch a subscription from Stripe (checkout and invoice events carry only an id). */
  fetchSubscription(id: string): Promise<Stripe.Subscription>;
}

/** What `handleEvent` decided, so a caller (and a test) can assert on it. */
export type HandleResult =
  | { kind: "applied"; userId: string }
  | { kind: "skipped"; reason: "stale" | "no-user" | "ignored-type" | "no-subscription" };

/** Pull the Supabase user id off the subscription, falling back to the customer. */
export async function resolveUserId(
  sub: Stripe.Subscription,
  ledger: Ledger,
): Promise<string | null> {
  const fromSub = sub.metadata?.supabase_user_id;
  if (fromSub) return fromSub;
  const customerId = typeof sub.customer === "string" ? sub.customer : sub.customer?.id;
  if (!customerId) return null;
  return await ledger.userForCustomer(customerId);
}

/**
 * Apply a subscription's state, unless the row already reflects something newer.
 *
 * `eventAt` is Stripe's own `created` for the event that carried this payload —
 * not our clock, and not the subscription's own timestamps, because what is being
 * ordered is the events themselves.
 */
export async function syncSubscription(
  sub: Stripe.Subscription,
  eventAt: number,
  ledger: Ledger,
): Promise<HandleResult> {
  const userId = await resolveUserId(sub, ledger);
  if (!userId) return { kind: "skipped", reason: "no-user" };

  // The ordering guard. A user we have never seen has no watermark, so a first
  // event always applies.
  const applied = await ledger.lastEventAt(userId);
  if (applied !== null && applied > eventAt) {
    return { kind: "skipped", reason: "stale" };
  }

  const item = sub.items.data[0];
  const priceId = item?.price?.id ?? null;
  const plan = (await ledger.planForPrice(priceId)) ?? "free";
  const canceled = sub.status === "canceled" || sub.status === "incomplete_expired";

  await ledger.upsertSubscription({
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
  });
  return { kind: "applied", userId };
}

/**
 * Handle one verified Stripe event.
 *
 * The caller has already checked the signature; this decides what the event
 * means. It does **not** claim the event — claiming has to happen before any
 * work, and therefore before this is called, so a crash in here cannot leave a
 * claimed-but-unapplied event behind.
 */
export async function handleEvent(
  event: Stripe.Event,
  ledger: Ledger,
): Promise<HandleResult> {
  switch (event.type) {
    case "customer.subscription.created":
    case "customer.subscription.updated":
    case "customer.subscription.deleted":
      return await syncSubscription(
        event.data.object as Stripe.Subscription,
        event.created,
        ledger,
      );

    case "checkout.session.completed": {
      // The subscription arrives in its own event too, but fetching it here means
      // the upgrade is reflected the instant checkout returns.
      const session = event.data.object as Stripe.Checkout.Session;
      if (!session.subscription) return { kind: "skipped", reason: "no-subscription" };
      const subId = typeof session.subscription === "string"
        ? session.subscription
        : session.subscription.id;
      const sub = await ledger.fetchSubscription(subId);
      return await syncSubscription(sub, event.created, ledger);
    }

    case "invoice.payment_failed": {
      // A failed payment does not cancel a subscription — Stripe keeps retrying
      // and the customer keeps their plan through the dunning window. But
      // `past_due` is what lets the account page say "your card was declined"
      // instead of leaving someone to discover it weeks later.
      const invoice = event.data.object as Stripe.Invoice;
      const subId = typeof invoice.subscription === "string"
        ? invoice.subscription
        : invoice.subscription?.id;
      if (!subId) return { kind: "skipped", reason: "no-subscription" };
      const sub = await ledger.fetchSubscription(subId);
      return await syncSubscription(sub, event.created, ledger);
    }

    default:
      return { kind: "skipped", reason: "ignored-type" };
  }
}
