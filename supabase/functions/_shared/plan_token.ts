// Signing the plan assertion the desktop app hands to its local proxy.
//
// # Why this exists
//
// The app read `plan_id` from /me and POSTed it to the proxy, which stored it
// verbatim. Nothing was signed, nothing expired, and nothing was bound to a user,
// so the proxy was not checking an entitlement so much as being told one.
//
// The token is Ed25519 over a compact payload: `base64url(json).base64url(sig)`.
// Deliberately not a JWT — exactly one algorithm is needed here, and JWT's `alg`
// header is a well-worn route to accepting `none`.
//
// # Key handling
//
// `ENTITLEMENT_SIGNING_KEY` is the 32-byte Ed25519 seed, base64url, set as a
// function secret. Its public half is compiled into release builds of the app as
// `DRIFTERR_ENTITLEMENT_PUBKEY`. Generate a pair with:
//
//   deno eval 'const k = await crypto.subtle.generateKey({name:"Ed25519"}, true, ["sign","verify"]);
//     const pk = new Uint8Array(await crypto.subtle.exportKey("raw", k.publicKey));
//     const sk = new Uint8Array((await crypto.subtle.exportKey("pkcs8", k.privateKey)).slice(-32));
//     const b = (u) => btoa(String.fromCharCode(...u)).replace(/\+/g,"-").replace(/\//g,"_").replace(/=+$/,"");
//     console.log("public :", b(pk)); console.log("private:", b(sk));'
//
// Rotation is additive on the app side (ship a build that accepts the new key,
// then switch the secret), so a rotation never strips a live customer of a plan.

const SEED_B64 = Deno.env.get("ENTITLEMENT_SIGNING_KEY") ?? "";

/// How long a signed plan is good for.
///
/// A day, not an hour: the app refreshes on launch, and a shorter window would
/// mean a laptop opened offline in the morning loses its paid features. Not a
/// week: a cancellation should stop mattering quickly.
const TTL_MS = 24 * 60 * 60 * 1000;

function b64urlDecode(s: string): Uint8Array {
  const pad = s.replace(/-/g, "+").replace(/_/g, "/");
  const bin = atob(pad + "=".repeat((4 - (pad.length % 4)) % 4));
  return Uint8Array.from(bin, (c) => c.charCodeAt(0));
}

function b64urlEncode(bytes: Uint8Array): string {
  return btoa(String.fromCharCode(...bytes))
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
}

let cachedKey: CryptoKey | null = null;

async function signingKey(): Promise<CryptoKey | null> {
  if (cachedKey) return cachedKey;
  if (!SEED_B64.trim()) return null;
  try {
    // Wrap the raw 32-byte seed in the minimal PKCS#8 envelope WebCrypto wants.
    const seed = b64urlDecode(SEED_B64.trim());
    if (seed.length !== 32) {
      console.error(
        "plan_token: ENTITLEMENT_SIGNING_KEY must decode to 32 bytes",
      );
      return null;
    }
    const pkcs8 = new Uint8Array([
      0x30,
      0x2e,
      0x02,
      0x01,
      0x00,
      0x30,
      0x05,
      0x06,
      0x03,
      0x2b,
      0x65,
      0x70,
      0x04,
      0x22,
      0x04,
      0x20,
      ...seed,
    ]);
    cachedKey = await crypto.subtle.importKey(
      "pkcs8",
      pkcs8,
      { name: "Ed25519" },
      false,
      [
        "sign",
      ],
    );
    return cachedKey;
  } catch (e) {
    console.error("plan_token: could not import signing key", e);
    return null;
  }
}

/**
 * Sign a plan assertion for `userId`.
 *
 * Returns `null` when no key is configured, and the caller omits the field —
 * a proxy that receives no token reports the plan as unverified, which is the
 * honest state rather than a silent downgrade or a fake signature.
 */
export async function signPlanToken(
  userId: string,
  planId: string,
): Promise<string | null> {
  const key = await signingKey();
  if (!key) return null;
  const payload = JSON.stringify({
    sub: userId,
    plan: planId,
    exp: Date.now() + TTL_MS,
    iat: Date.now(),
  });
  const encodedPayload = b64urlEncode(new TextEncoder().encode(payload));
  // Sign the *encoded* payload, so verification never depends on two JSON
  // writers agreeing byte for byte.
  const sig = new Uint8Array(
    await crypto.subtle.sign(
      { name: "Ed25519" },
      key,
      new TextEncoder().encode(encodedPayload),
    ),
  );
  return `${encodedPayload}.${b64urlEncode(sig)}`;
}
