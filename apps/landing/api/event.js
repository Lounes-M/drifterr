// First-party analytics collector for the marketing site.
//
// Deliberately dumb and deliberately ours. It exists so the funnel is measurable
// without handing a third-party script a view of our visitors — and so nothing
// about the *app* is involved: this endpoint only ever hears about page and button
// events from the public website (see ../analytics.js for the boundary).
//
// What it does NOT do, by construction:
//   * set a cookie, or read one;
//   * store an IP address, a user agent, or any per-visitor identifier;
//   * accept free-text — the event name and every property is allowlisted, and
//     anything unrecognized is dropped rather than stored.
//
// Where the counts go: by default, one structured line to the platform log, which
// is queryable on day one and costs nothing to set up. Point
// `ANALYTICS_FORWARD_URL` at an aggregate sink (Plausible/Umami-compatible, or
// your own) to also forward there. Either way the payload is the same handful of
// enum values assembled below.

import { analytics } from "./_ratelimit.js";

const ALLOWED_EVENTS = new Set([
  "page_view",
  "download_click",
  "download_started",
  "download_failed",
  "install_help_view",
  "pricing_view",
  "plan_click",
  "signup_start",
  "faq_open",
]);

const ALLOWED_OS = new Set(["mac", "win", "linux", "deb"]);
const ALLOWED_PLAN = new Set(["free", "pro", "team"]);
const ALLOWED_PAGE = new Set([
  "home",
  "download",
  "pricing",
  "proof",
  "signup",
  "login",
  "account",
  "other",
]);

const MAX_BODY_BYTES = 2048;

/// Re-validate everything server-side. The client sanitizes too, but a public
/// endpoint must never trust its caller — that's how a metrics pipe turns into an
/// unbounded free-text log.
function clean(payload) {
  const name = String(payload?.e || "");
  if (!ALLOWED_EVENTS.has(name)) return null;

  const props = {};
  const p = payload?.p || {};
  if (ALLOWED_OS.has(p.os)) props.os = p.os;
  if (ALLOWED_PLAN.has(p.plan)) props.plan = p.plan;
  if (Number.isInteger(p.index) && p.index >= 0 && p.index < 100) props.index = p.index;

  const page = ALLOWED_PAGE.has(payload?.pg) ? payload.pg : "other";
  // Referrer host only, length-capped, and only if it looks like a hostname.
  const rf = String(payload?.rf || "");
  const referrer = /^[a-z0-9.-]{1,80}$/i.test(rf) ? rf.toLowerCase() : "";

  return { event: name, page, referrer, ...props };
}

async function readBody(req) {
  // Vercel usually parses JSON for us; fall back to reading the stream, capped.
  if (req.body && typeof req.body === "object") return req.body;
  if (typeof req.body === "string") {
    return req.body.length > MAX_BODY_BYTES ? null : JSON.parse(req.body);
  }
  let size = 0;
  const chunks = [];
  for await (const chunk of req) {
    size += chunk.length;
    if (size > MAX_BODY_BYTES) return null;
    chunks.push(chunk);
  }
  if (!chunks.length) return null;
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

export default async function handler(req, res) {
  // Always answer 204: a metrics endpoint must never surface an error to a
  // visitor's console, and must never tell a prober what it accepts.
  const done = () => {
    res.statusCode = 204;
    // Never cache, never let a CDN coalesce these.
    res.setHeader("Cache-Control", "no-store");
    return res.end();
  };

  if (req.method !== "POST") return done();

  let payload;
  try {
    payload = await readBody(req);
  } catch {
    return done();
  }

  const evt = clean(payload);
  if (!evt) return done();

  // Shed load rather than forward it. The bucket is global, not per-visitor —
  // see _ratelimit.js for why keying on IP would contradict this endpoint's own
  // no-visitor-id promise. Dropping is silent to the caller (a page view can do
  // nothing about being throttled) but counted in the log, because a throttle
  // that leaves no trace turns a traffic cliff into a mystery.
  if (!analytics.take()) {
    const dropped = analytics.drainDropped();
    if (dropped % 100 === 1) {
      // eslint-disable-next-line no-console -- the platform log IS the sink.
      console.warn(`analytics rate limit: dropped ${dropped} event(s)`);
    }
    return done();
  }

  // Day-of-month granularity only: enough to chart a funnel, too coarse to
  // correlate one visitor's events with another's.
  const record = { ...evt, day: new Date().toISOString().slice(0, 10) };

  // eslint-disable-next-line no-console -- the platform log IS the default sink.
  console.log(`analytics ${JSON.stringify(record)}`);

  const forward = process.env.ANALYTICS_FORWARD_URL;
  if (forward) {
    try {
      await fetch(forward, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          ...(process.env.ANALYTICS_FORWARD_TOKEN
            ? { Authorization: `Bearer ${process.env.ANALYTICS_FORWARD_TOKEN}` }
            : {}),
        },
        body: JSON.stringify(record),
      });
    } catch {
      // A sink being down must never turn into a failed request for the visitor.
    }
  }

  return done();
}
