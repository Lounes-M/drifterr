//! Fuzz-lite: the response/request parsers must never panic.
//!
//! The proxy's #1 rule is that it never breaks a user's request. Parsing runs
//! **off** the response path and is best-effort — but "best-effort" only holds
//! if it cannot panic on hostile or truncated input (a provider mid-stream, a
//! garbage body, invalid UTF-8, a half-written SSE frame). This throws thousands
//! of adversarial inputs at both parsers, for both schemas and assorted
//! content-types, and asserts the process survives (a panic fails the test).
//!
//! Deterministic (seeded SplitMix64) so a failure is reproducible. For true
//! coverage-guided fuzzing, a `cargo-fuzz` target over these same entry points
//! is the next step; this is the always-on CI guard.

use drifterr_proxy::provider::{parse_request, parse_response, Provider};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn range(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Fragments that look enough like real SSE / JSON to drive the parser deep
/// before hitting the malformed part.
const FRAGMENTS: &[&[u8]] = &[
    b"data: {\"choices\":[{\"delta\":{\"content\":\"",
    b"\"}}]}\n\n",
    b"data: [DONE]\n\n",
    b"event: content_block_delta\n",
    b"data: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"",
    b"{\"usage\":{\"prompt_tokens\":",
    b"\"completion_tokens\":999999999999999999",
    b"}}",
    b"\n\n",
    b"\0\0\0",
    b"\xff\xfe\xfd", // invalid UTF-8
    b"{\"messages\":[{\"role\":\"user\",\"content\":",
    b"]}",
    b"stream",
    b"\"model\":\"gpt-4o\"",
    b"      ",
];

fn build_input(rng: &mut Rng) -> Vec<u8> {
    let mut out = Vec::new();
    match rng.range(4) {
        // pure random bytes
        0 => {
            let len = rng.range(4096);
            for _ in 0..len {
                out.push((rng.next() & 0xff) as u8);
            }
        }
        // a pile of SSE/JSON fragments, possibly truncated
        1 | 2 => {
            let n = rng.range(40);
            for _ in 0..n {
                out.extend_from_slice(FRAGMENTS[rng.range(FRAGMENTS.len())]);
            }
            // random truncation
            let keep = if out.is_empty() {
                0
            } else {
                rng.range(out.len() + 1)
            };
            out.truncate(keep);
        }
        // fragments interleaved with random bytes
        _ => {
            let n = rng.range(20);
            for _ in 0..n {
                out.extend_from_slice(FRAGMENTS[rng.range(FRAGMENTS.len())]);
                for _ in 0..rng.range(8) {
                    out.push((rng.next() & 0xff) as u8);
                }
            }
        }
    }
    out
}

#[test]
fn parsers_never_panic_on_hostile_input() {
    let providers = [Provider::OpenAI, Provider::Anthropic];
    let content_types = [
        "text/event-stream",
        "application/json",
        "",
        "text/plain; charset=utf-8",
        "garbage/nonsense",
    ];

    let mut rng = Rng(0xF0F0_1234_ABCD_5678);
    for _ in 0..20_000 {
        let input = build_input(&mut rng);
        let provider = providers[rng.range(providers.len())];

        // Request parser: hostile bodies must degrade, not crash.
        let _ = parse_request(provider, &input);

        // Response parser: every content-type, both schemas.
        let ct = content_types[rng.range(content_types.len())];
        let parsed = parse_response(provider, ct, &input);
        // Sanity: token fields, if present, are just numbers we carried through —
        // nothing here should be able to make a later consumer panic. Touch them.
        let _ = parsed.input_tokens.map(|t| t.saturating_add(1));
        let _ = parsed.output_tokens.map(|t| t.saturating_add(1));
        let _ = parsed.assistant_text.len();
    }
    // Reaching here = 20k adversarial inputs parsed without a panic.
}

#[test]
fn empty_and_boundary_inputs_are_safe() {
    for provider in [Provider::OpenAI, Provider::Anthropic] {
        let _ = parse_request(provider, b"");
        let _ = parse_response(provider, "", b"");
        let _ = parse_response(provider, "text/event-stream", b"data: ");
        let _ = parse_response(provider, "application/json", b"{");
        let _ = parse_response(provider, "text/event-stream", b"data: [DONE]\n\n");
        // A huge single frame.
        let big = format!(
            "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{}\"}}}}]}}\n\n",
            "x".repeat(200_000)
        );
        let _ = parse_response(provider, "text/event-stream", big.as_bytes());
    }
}
