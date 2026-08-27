//! The transparent relay — the one part of Drifterr on the user's request path.
//!
//! Split out of `lib.rs` so the rule that governs it is impossible to miss: the
//! upstream byte stream is forwarded to the client **unchanged**, and detection
//! runs off a cheap tee after the stream ends. A monitoring tool that can break
//! your request is worse than no monitoring tool, so nothing in this file may
//! buffer, rewrite or fail a response.
//!
//! Unlike the control API next door, this router is deliberately unauthenticated:
//! it is a drop-in replacement for a provider's base URL, and a tool pointed at it
//! sends the user's own provider key, not ours.

use super::*;

/// The transparent relay: a catch-all over every path and method.
pub fn proxy_router(state: AppState) -> Router {
    Router::new().fallback(proxy_handler).with_state(state)
}

/// The transparent relay handler with the streaming tee.
async fn proxy_handler(State(app): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());

    // We must read the whole *request* body — both to relay it and to recover
    // the conversation. (Requests are not the streaming concern; responses are.)
    let body_bytes = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "request body too large"),
    };

    let provider = Provider::from_path(&path_and_query);
    // Read the live upstream (runtime-switchable via the provider selector).
    let (base, strip_v1) = match app.upstream.read() {
        Ok(u) => match provider {
            Provider::OpenAI => (u.openai_upstream.clone(), u.openai_strip_v1),
            Provider::Anthropic => (u.anthropic_upstream.clone(), false),
        },
        Err(_) => (app.cfg.upstream_for(provider).to_string(), false),
    };
    // Gemini-style upstreams carry their own `/v1beta/openai` prefix, so strip the
    // incoming `/v1` for OpenAI-schema traffic when configured.
    let strip = matches!(provider, Provider::OpenAI) && strip_v1;
    let url = upstreams::join_url(&base, &path_and_query, strip);
    let parsed_req = provider::parse_request(provider, &body_bytes);

    // Opt-in auto-re-anchor: if this session is currently drifting (RED), inject
    // the re-anchor preamble into the outgoing request. Idempotent and best-
    // effort — on any doubt we relay the original bytes unchanged.
    let mut body_to_send = body_bytes.to_vec();
    if app.auto_reanchor_on() && app.entitlement().auto_reanchor {
        let session_id = state::session_id_for(&parsed_req);
        let preamble = app
            .core
            .lock()
            .ok()
            .and_then(|core| core.auto_preamble(&session_id));
        if let Some(preamble) = preamble {
            if let Some(modified) = provider::inject_preamble(provider, &body_bytes, &preamble) {
                body_to_send = modified;
            }
        }
    }

    // Relay to the real provider.
    let upstream = app
        .client
        .request(parts.method.clone(), &url)
        .headers(forward_headers(&parts.headers))
        .body(body_to_send)
        .send()
        .await;
    let upstream = match upstream {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_GATEWAY, &format!("upstream error: {e}")),
    };

    let status = upstream.status();
    let resp_headers = upstream.headers().clone();
    let content_type = resp_headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // The tee: each chunk is cloned (a refcount bump on `Bytes`) into a channel,
    // then yielded onward to the client untouched.
    let (tee_tx, mut tee_rx) = tokio::sync::mpsc::unbounded_channel::<Bytes>();
    let teed = upstream.bytes_stream().map(move |chunk| {
        if let Ok(bytes) = &chunk {
            let _ = tee_tx.send(bytes.clone());
        }
        chunk
    });

    // Detection runs entirely off the response path. When the client finishes
    // (or disconnects) the stream drops, closing the channel, and this task
    // finalizes with whatever was received.
    let app2 = app.clone();
    tokio::spawn(async move {
        let mut buf = Vec::new();
        while let Some(b) = tee_rx.recv().await {
            buf.extend_from_slice(&b);
        }
        let parsed_resp = provider::parse_response(provider, &content_type, &buf);
        if parsed_resp.assistant_text.is_empty() && !parsed_resp.has_exact_usage() {
            return; // nothing detectable in this response
        }
        let session_id = state::session_id_for(&parsed_req);

        // Base (deterministic + soft) signals — sync, under the lock.
        let decisions = {
            let Ok(mut core) = app2.core.lock() else {
                return;
            };
            core.record_turn(&session_id, &parsed_req, &parsed_resp);
            core.decisions_for(&session_id)
        };

        // Judge phase — async, off the lock. It powers the *fuzzy* checks the
        // deterministic engine can't make: Signal 3 (decision coherence) and the
        // judge half of Signal 1 (fuzzy constraint adherence). All of it is
        // fail-safe and AMBER-only — a judge that cries wolf can only raise a
        // watch, never a wall.
        // Snapshot the judge (cheap clone) so the runtime-swappable RwLock isn't
        // held across the awaits below.
        let judge = match app2.judge.read() {
            Ok(j) => j.clone(),
            Err(_) => return,
        };
        if !judge.enabled() || parsed_resp.assistant_text.is_empty() {
            return;
        }

        let last = drifterr_engine::conversation::Turn {
            index: parsed_req.turns.len(),
            role: drifterr_engine::conversation::Role::Assistant,
            content: parsed_resp.assistant_text.clone(),
            tokens: parsed_resp.output_tokens.unwrap_or(0),
            timestamp: 0,
        };

        // (a) LLM-assisted constraint extraction. Gate on a cheap local cue so we
        // only spend a call on the newest user turn when it plausibly states a
        // rule; add whatever fuzzy constraints it yields to the baseline.
        if let Some(user_msg) = parsed_req
            .turns
            .iter()
            .rev()
            .find(|t| t.role == drifterr_engine::conversation::Role::User)
        {
            if drifterr_engine::infer::has_constraint_cue(&user_msg.content) {
                let extracted = judge.extract_constraints(&user_msg.content).await;
                if !extracted.is_empty() {
                    if let Ok(mut core) = app2.core.lock() {
                        core.add_judge_constraints(&session_id, extracted);
                    }
                }
            }
        }

        // (a2) Auto-intent: infer the whole intent (goal + constraints) from the
        // conversation so the user never has to type it. Opt-in, rate-limited, and
        // fail-safe — an empty/failed inference just advances the rate limiter. The
        // goal it sets only feeds the soft signal; a big goal shift is surfaced as
        // a prompt, never a silent overwrite (see apply_inferred_intent).
        if app2.auto_intent_on() {
            // Cost control, cheapest checks first (BYOK — the user pays):
            //   1. cadence + per-session budget (no transcript needed);
            //   2. cache — skip the call if the transcript hasn't changed.
            let due = app2
                .core
                .lock()
                .map(|c| c.due_for_intent_synthesis(&session_id))
                .unwrap_or(false);
            if due {
                let transcript = state::transcript_for(&parsed_req, &parsed_resp);
                let hash = state::transcript_digest(&transcript);
                let changed = app2
                    .core
                    .lock()
                    .map(|c| c.synth_content_changed(&session_id, hash))
                    .unwrap_or(false);
                if changed {
                    let intent = judge
                        .synthesize_intent(&transcript)
                        .await
                        .unwrap_or_default();
                    if let Ok(mut core) = app2.core.lock() {
                        core.apply_inferred_intent(&session_id, &intent, hash);
                    }
                } else if let Ok(mut core) = app2.core.lock() {
                    // Unchanged transcript — advance cadence without a paid call.
                    core.note_synth_skipped(&session_id);
                }
            }
        }

        // (b) Run both judge signals against the new assistant turn, then merge
        // their events in one pass so the status updates once.
        let judge_constraints = {
            let Ok(core) = app2.core.lock() else {
                return;
            };
            core.judge_constraints_for(&session_id)
        };
        let embedder = drifterr_embeddings::BagEmbedder::default();

        let mut extra =
            drifterr_judge::constraint::constraint_adherence(&last, &judge_constraints, &judge)
                .await;

        if !decisions.is_empty() {
            if let Some(event) =
                drifterr_judge::decision::decision_coherence(&last, &decisions, &embedder, &judge)
                    .await
            {
                extra.push(event);
            }
        }

        if !extra.is_empty() {
            if let Ok(mut core) = app2.core.lock() {
                core.apply_extra_events(&session_id, extra);
            }
        }
    });

    // Relay status + headers + the streaming body.
    let mut builder = Response::builder().status(status.as_u16());
    for (name, value) in resp_headers.iter() {
        let n = name.as_str();
        if is_hop_by_hop(n) || n == "content-length" {
            continue; // length is unknown for a stream; let the server frame it
        }
        builder = builder.header(name.as_str(), value.as_bytes());
    }
    builder.body(Body::from_stream(teed)).unwrap_or_else(|_| {
        error_response(StatusCode::INTERNAL_SERVER_ERROR, "response build error")
    })
}

/// Copy request headers for forwarding, dropping hop-by-hop headers, `host`
/// (reqwest sets it for the upstream), `content-length` (reqwest recomputes it),
/// and `accept-encoding` (so the upstream returns identity bytes we can parse —
/// the client still receives whatever the upstream sends).
fn forward_headers(src: &axum::http::HeaderMap) -> reqwest::header::HeaderMap {
    let mut out = reqwest::header::HeaderMap::new();
    for (name, value) in src.iter() {
        let n = name.as_str();
        if is_hop_by_hop(n) || n == "host" || n == "content-length" || n == "accept-encoding" {
            continue;
        }
        if let (Ok(hn), Ok(hv)) = (
            reqwest::header::HeaderName::from_bytes(n.as_bytes()),
            reqwest::header::HeaderValue::from_bytes(value.as_bytes()),
        ) {
            out.insert(hn, hv);
        }
    }
    out
}

/// RFC 7230 hop-by-hop headers (must not be forwarded by a proxy).
fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn error_response(code: StatusCode, msg: &str) -> Response {
    Response::builder()
        .status(code)
        .header("content-type", "text/plain")
        .body(Body::from(format!("drifterr proxy: {msg}")))
        .expect("error response")
}
