//! Verifying a signed entitlement, so the proxy checks a plan rather than trusting one.
//!
//! # What was wrong
//!
//! The panel fetched the plan from Supabase `/me` and then *told* the local proxy
//! what it was: `POST /entitlement {"plan":"team"}`, stored verbatim. There was no
//! signature, no expiry, nothing bound to a user. One unauthenticated request
//! granted Team, and since the source is public the endpoint was discoverable by
//! reading.
//!
//! Authenticating the control API closed the remote half of that — a website can
//! no longer reach the endpoint at all. What remained was that the proxy took the
//! panel's word for it, which is not a boundary so much as an honour system with a
//! JSON schema.
//!
//! # What this is, and what it honestly is not
//!
//! The backend signs a short-lived assertion — *this user, this plan, expiring
//! then* — with a key only it holds. The proxy verifies it against a public key
//! compiled into the build. A caller can no longer assert a plan; it has to
//! present one the server signed.
//!
//! It does **not** make entitlement tamper-proof, and nothing can: this is local
//! software, the user owns the machine, and a determined user can patch the binary
//! or the public key. That is a property of the category, not a gap in this file,
//! and [`docs/ACCOUNTS.md`] says so in the same words rather than implying a
//! guarantee we cannot make. What it does buy is real: no accidental grants, no
//! scriptable bypass, no stale entitlement surviving a cancellation for longer than
//! the token's lifetime, and a single place where "what plan is this?" is answered.
//!
//! # Offline
//!
//! A token carries an expiry, so a signed-in user keeps their plan across a flight
//! or a Supabase outage until it lapses — deliberately, because a product whose
//! paid features evaporate when the network hiccups is worse than one that trusts
//! a week-old signature.
//!
//! # Not configured
//!
//! With no public key compiled in — a development build, a self-hoster — the plain
//! `{"plan": …}` path still works and `GET /entitlement` reports
//! `"verified": false`. That is honest rather than lenient: the state is visible,
//! and a release build ships the key, so the shipped product does verify.

use ed25519_dalek::{Signature, VerifyingKey};

/// Compile-time public key (base64url, unpadded, 32 bytes decoded), injected by
/// the release build via `DRIFTERR_ENTITLEMENT_PUBKEY`. Empty in a plain
/// `cargo build`, which is what leaves the unverified path available for
/// development.
const BUILD_PUBKEY: Option<&str> = option_env!("DRIFTERR_ENTITLEMENT_PUBKEY");

/// The verified contents of a plan token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanClaims {
    /// The account the plan belongs to.
    pub subject: String,
    /// The plan id, exactly as `entitlement::Plan::from_id` reads it.
    pub plan: String,
    /// Expiry, milliseconds since the epoch.
    pub expires_ms: i64,
}

/// Why a token was refused. Distinct variants because they need distinct answers:
/// an expired token means "sign in again", a bad signature means something is
/// wrong, and no key configured means this build does not verify at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanTokenError {
    /// No public key compiled in — this build cannot verify anything.
    NotConfigured,
    /// Structurally not a token.
    Malformed,
    /// Signature did not verify against the configured key.
    BadSignature,
    /// Verified, but past its expiry.
    Expired,
}

impl std::fmt::Display for PlanTokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            PlanTokenError::NotConfigured => "this build has no entitlement key configured",
            PlanTokenError::Malformed => "the plan token is malformed",
            PlanTokenError::BadSignature => "the plan token's signature did not verify",
            PlanTokenError::Expired => "the plan token has expired — sign in again",
        };
        f.write_str(s)
    }
}

/// Is this build able to verify plan tokens at all?
///
/// Surfaced at `GET /entitlement` as `verified`, so the panel and a support
/// conversation can both tell "verified Pro" from "asserted Pro" without guessing.
pub fn verification_available() -> bool {
    configured_key().is_some()
}

fn configured_key() -> Option<VerifyingKey> {
    // The environment override exists for tests and for a self-hosted backend with
    // its own signing key. It is read at call time rather than cached so a test can
    // set it without a process restart.
    let encoded = std::env::var("DRIFTERR_ENTITLEMENT_PUBKEY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| BUILD_PUBKEY.map(str::to_string))
        .filter(|s| !s.trim().is_empty())?;
    let bytes = b64url_decode(encoded.trim())?;
    let arr: [u8; 32] = bytes.try_into().ok()?;
    VerifyingKey::from_bytes(&arr).ok()
}

/// Verify a plan token and return its claims.
///
/// Format is `base64url(json).base64url(sig)` — compact, URL-safe, and small
/// enough to sit in a JSON field. Deliberately not JWT: we need exactly one
/// algorithm, and JWT's `alg` header is a well-worn way to end up accepting
/// `none`.
pub fn verify(token: &str, now_ms: i64) -> Result<PlanClaims, PlanTokenError> {
    let key = configured_key().ok_or(PlanTokenError::NotConfigured)?;

    let (payload_b64, sig_b64) = token
        .trim()
        .split_once('.')
        .ok_or(PlanTokenError::Malformed)?;
    let payload = b64url_decode(payload_b64).ok_or(PlanTokenError::Malformed)?;
    let sig_bytes = b64url_decode(sig_b64).ok_or(PlanTokenError::Malformed)?;
    let sig_arr: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| PlanTokenError::Malformed)?;

    // Verify over the *encoded* payload, not the decoded JSON: re-serializing
    // first would make the signature depend on our JSON writer agreeing with the
    // signer's, byte for byte, forever.
    key.verify_strict(payload_b64.as_bytes(), &Signature::from_bytes(&sig_arr))
        .map_err(|_| PlanTokenError::BadSignature)?;

    let v: serde_json::Value =
        serde_json::from_slice(&payload).map_err(|_| PlanTokenError::Malformed)?;
    let claims = PlanClaims {
        subject: v
            .get("sub")
            .and_then(|x| x.as_str())
            .ok_or(PlanTokenError::Malformed)?
            .to_string(),
        plan: v
            .get("plan")
            .and_then(|x| x.as_str())
            .ok_or(PlanTokenError::Malformed)?
            .to_string(),
        expires_ms: v
            .get("exp")
            .and_then(|x| x.as_i64())
            .ok_or(PlanTokenError::Malformed)?,
    };
    if claims.expires_ms <= now_ms {
        return Err(PlanTokenError::Expired);
    }
    Ok(claims)
}

/// Decode unpadded base64url. Hand-rolled because the only alternative is another
/// dependency for forty lines of table lookup, and this one has an exhaustive test
/// below rather than a promise.
fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            // Padding is tolerated but not required; anything else is not base64url.
            b'=' => continue,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    /// A fixed keypair, so the tests are reproducible and never touch an RNG.
    fn keypair() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn b64url(bytes: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            let take = chunk.len() + 1;
            for i in 0..take {
                out.push(T[((n >> (18 - 6 * i)) & 0x3f) as usize] as char);
            }
        }
        out
    }

    fn mint(plan: &str, exp_ms: i64) -> String {
        let sk = keypair();
        let payload = format!(r#"{{"sub":"user-1","plan":"{plan}","exp":{exp_ms}}}"#);
        let payload_b64 = b64url(payload.as_bytes());
        let sig = sk.sign(payload_b64.as_bytes());
        format!("{payload_b64}.{}", b64url(&sig.to_bytes()))
    }

    /// The configured key lives in a process-wide environment variable, so these
    /// tests must not run concurrently with each other — one removing the key while
    /// another is verifying is a flake, not a finding.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_key<T>(f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let vk = keypair().verifying_key();
        std::env::set_var("DRIFTERR_ENTITLEMENT_PUBKEY", b64url(vk.as_bytes()));
        let out = f();
        std::env::remove_var("DRIFTERR_ENTITLEMENT_PUBKEY");
        out
    }

    #[test]
    fn a_valid_token_verifies_and_carries_its_claims() {
        with_key(|| {
            let claims = verify(&mint("pro", 2_000), 1_000).expect("should verify");
            assert_eq!(claims.plan, "pro");
            assert_eq!(claims.subject, "user-1");
            assert_eq!(claims.expires_ms, 2_000);
        });
    }

    /// The whole point: a plan the server did not sign must not be accepted, and
    /// flipping a byte of a real token must not either.
    #[test]
    fn a_forged_or_tampered_token_is_refused() {
        with_key(|| {
            let good = mint("free", 2_000);

            // Re-signed with a different key.
            let other = SigningKey::from_bytes(&[9u8; 32]);
            let payload = b64url(br#"{"sub":"user-1","plan":"team","exp":2000}"#);
            let forged = format!(
                "{payload}.{}",
                b64url(&other.sign(payload.as_bytes()).to_bytes())
            );
            assert_eq!(verify(&forged, 1_000), Err(PlanTokenError::BadSignature));

            // Payload edited, signature left alone — the classic attempt.
            let (_, sig) = good.split_once('.').unwrap();
            let swapped = format!(
                "{}.{sig}",
                b64url(br#"{"sub":"user-1","plan":"team","exp":2000}"#)
            );
            assert_eq!(verify(&swapped, 1_000), Err(PlanTokenError::BadSignature));

            // Structural nonsense.
            for bad in ["", "nope", "a.b", "....", "!!!.???"] {
                assert!(
                    matches!(
                        verify(bad, 1_000),
                        Err(PlanTokenError::Malformed) | Err(PlanTokenError::BadSignature)
                    ),
                    "{bad:?} must not verify"
                );
            }
        });
    }

    /// An expiry that is not enforced is a comment.
    #[test]
    fn an_expired_token_is_refused() {
        with_key(|| {
            assert_eq!(
                verify(&mint("pro", 1_000), 1_000),
                Err(PlanTokenError::Expired)
            );
            assert_eq!(
                verify(&mint("pro", 999), 1_000),
                Err(PlanTokenError::Expired)
            );
            assert!(verify(&mint("pro", 1_001), 1_000).is_ok());
        });
    }

    /// A build with no key says so, rather than silently accepting anything.
    #[test]
    fn without_a_key_nothing_verifies_and_the_state_is_visible() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("DRIFTERR_ENTITLEMENT_PUBKEY");
        if BUILD_PUBKEY.unwrap_or("").is_empty() {
            assert!(!verification_available());
            assert_eq!(
                verify(&mint("pro", i64::MAX), 0),
                Err(PlanTokenError::NotConfigured)
            );
        }
    }

    #[test]
    fn base64url_round_trips() {
        for len in 0..40usize {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 % 251) as u8).collect();
            assert_eq!(
                b64url_decode(&b64url(&bytes)).as_deref(),
                Some(bytes.as_slice()),
                "round trip failed at length {len}"
            );
        }
        assert_eq!(b64url_decode("not base64!"), None);
    }
}
