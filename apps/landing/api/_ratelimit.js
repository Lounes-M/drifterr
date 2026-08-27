// A cap on how much work the public endpoints will do.
//
// # Why this is deliberately not per-visitor
//
// The obvious rate limiter keys on IP address. This one cannot, and the reason
// is the product: `/api/event` is the analytics collector whose stated contract
// is "no cookie, no visitor id, no IP stored — events are counted, not people"
// (see ../analytics.js and /proof). Keying a limiter on IP would create exactly
// the per-visitor identifier the page promises does not exist, in the one file
// where it would be least defensible. A privacy claim that quietly acquires an
// exception for the convenient case is not a privacy claim.
//
// So this is a **global** bucket: it caps total throughput per instance rather
// than any individual's share. That is a worse limiter and an honest one.
//
// # What it does and does not protect
//
// It protects the downstream sink and the log from a flood, and it bounds the
// cost of a loop pointed at the endpoint. It is per-instance and in-memory, so a
// platform that scales out gives an attacker one bucket per instance — which
// means it is a cost control, not a security boundary. The real defence against
// a determined flood is the platform's own edge (Vercel's DDoS protection, or a
// WAF rule), and this file does not pretend otherwise.
//
// A dropped event is silently discarded rather than answered with an error: the
// caller is a page view, there is nothing for it to do about being throttled,
// and a 429 in a visitor's console would be worse than a missing data point.

/**
 * A token bucket.
 *
 * `capacity` tokens, refilled at `perSecond`. Bursts up to capacity are fine —
 * a page that fires a view plus two clicks in a second is normal traffic, and a
 * limiter that punished it would be measuring nothing.
 */
export function bucket({ capacity, perSecond }) {
  let tokens = capacity;
  let last = Date.now();
  let dropped = 0;

  return {
    /** Take a token. False means the caller should drop this request. */
    take() {
      const now = Date.now();
      tokens = Math.min(capacity, tokens + ((now - last) / 1000) * perSecond);
      last = now;
      if (tokens < 1) {
        dropped++;
        return false;
      }
      tokens -= 1;
      return true;
    },
    /**
     * How many have been dropped since the last call, and reset.
     *
     * Silent dropping is right for the caller and wrong for us: a throttle that
     * leaves no trace turns "our traffic fell off a cliff" into an unsolvable
     * mystery. The count goes to the platform log, never to the sink.
     */
    drainDropped() {
      const n = dropped;
      dropped = 0;
      return n;
    },
  };
}

/**
 * The shared limit for the analytics collector.
 *
 * 20/second sustained with a burst of 60. A real page produces a handful of
 * events per visit, so this is roughly a thousand concurrent visitors before a
 * single instance starts shedding — far above real traffic and far below what a
 * loop can generate.
 */
export const analytics = bucket({ capacity: 60, perSecond: 20 });

/**
 * The download redirect. Cheaper than analytics (one lookup, one 302) but it is
 * the endpoint most worth pointing a loop at, since each hit is a release-asset
 * URL. Same shape, more headroom.
 */
export const downloads = bucket({ capacity: 120, perSecond: 40 });
