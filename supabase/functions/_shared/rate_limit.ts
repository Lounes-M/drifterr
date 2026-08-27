// Per-user call budgets for the authenticated edge functions.
//
// `stripe-checkout` is authenticated but was unbounded: a signed-in caller could
// create unlimited Checkout Sessions, each one an API call against our Stripe
// account and its rate limit, at no cost to them.
//
// The counter lives in Postgres, not in memory, because edge functions scale out
// — an in-process counter hands an attacker one bucket per instance. And the
// increment happens inside a single SQL statement, because read-then-write would
// let two concurrent calls both read the old count and both write count+1, so a
// limit of five would pass ten under exactly the load it exists to handle.
//
// Keying per-caller is fine here, unlike on the marketing site: these endpoints
// already require a JWT, so no new identifier is created by counting.

import type { SupabaseClient } from "npm:@supabase/supabase-js@2.47.10";

/** Budgets, in calls per window. Generous — this stops loops, not customers. */
export const LIMITS = {
  /** Buying is rare and deliberate; ten attempts an hour is a lot of indecision. */
  checkout: { limit: 10, window: "1 hour" },
  /** Opening the billing portal is cheap but still a Stripe call. */
  portal: { limit: 20, window: "1 hour" },
} as const;

/**
 * Consume one unit of `action`'s budget for `userId`.
 *
 * Returns true when the call may proceed.
 *
 * **Fails open.** If the limiter itself errors — the migration has not run, the
 * database is briefly unreachable — the call is allowed and the failure is
 * logged. A rate limiter that takes checkout down when it breaks has done more
 * damage than the abuse it was guarding against.
 */
export async function allow(
  admin: SupabaseClient,
  userId: string,
  action: keyof typeof LIMITS,
): Promise<boolean> {
  const { limit, window } = LIMITS[action];
  try {
    const { data, error } = await admin.rpc("consume_rate_limit", {
      p_subject: userId,
      p_action: action,
      p_limit: limit,
      p_window: window,
    });
    if (error) {
      console.error(`rate_limit: ${action} check failed, allowing`, error);
      return true;
    }
    return data !== false;
  } catch (e) {
    console.error(`rate_limit: ${action} check threw, allowing`, e);
    return true;
  }
}
