//! Team sharing — shared rule packs and metadata-only aggregates.
//!
//! # The bright line, and why this module exists to hold it
//!
//! Everything else in Drifterr is local. Team is the first capability that asks for
//! something to leave a laptop, so it is also the first place the local-first promise
//! could be broken by accident. This module exists so that the *only* thing which can be
//! shared is constructed in one place, by one function, under a filter that is tested.
//!
//! Two categories may be shared, and nothing else:
//!
//! * **Rule packs** — config the user wrote or forked. Text like "Never use `any` types".
//! * **Rule counts** — "this rule was flagged 7 times in the last 14 days", per rule id.
//!
//! Categorically excluded, and each for a specific reason:
//!
//! * **Spans.** A span is a literal excerpt of the model's output. It is chat content
//!   wearing a short name.
//! * **Goals.** The goal is derived from the user's own first message.
//! * **Session ids, file paths, model names, prompt text, turn counts, timestamps finer
//!   than a day.** Each is either chat-derived or a fingerprint of what someone is
//!   working on.
//! * **Session-inferred rule ids** (`c1`, `c2`). This one is subtle and is the reason
//!   [`is_pack_scoped`] exists: those ids name a constraint that was *mined from the user's
//!   own messages*, so even publishing the id-with-count leaks that the user said
//!   something the engine turned into a rule. Only pack-scoped ids (`pack:rule`) — which
//!   refer to shared config both sides already have — may be reported. Everything else is
//!   dropped, and the drop is *counted and shown*, never silent.
//!
//! # Why this module makes no network call
//!
//! `tests/egress.rs` forbids the proxy crate from carrying a backend hostname, and that
//! invariant is worth more than the convenience of uploading from here. So this module
//! only ever *builds and shows* the payload; the upload is performed by the layer that
//! already holds the user's account session (the desktop shell / web dashboard). The crate
//! that can see chat content therefore still cannot reach anything but the model provider,
//! structurally, and no reviewer has to take that on trust.
//!
//! The practical consequence is a good one: the user can see exactly what would be
//! uploaded, byte for byte, before anything is.

use drifterr_engine::pack::Pack;
use serde::{Deserialize, Serialize};

/// Longest either half of a rule id may be. Generous for a slug, short enough that an id
/// cannot be smuggled in as a payload.
const MAX_ID_SEGMENT: usize = 64;

/// Whether a rule id is pack-scoped (`pack-id:rule-id`) and therefore shareable.
///
/// Deliberately narrow and hand-rolled rather than a regex: this is the single predicate
/// standing between a teammate's dashboard and a rule the user only ever said out loud,
/// so it should be readable in full at the point of decision. Both halves must be
/// non-empty lowercase slugs — anything else (a session-local `c1`, an empty half, a
/// path, whitespace, uppercase) is not a pack rule and is withheld.
fn is_pack_scoped(id: &str) -> bool {
    let mut halves = id.split(':');
    let (Some(pack), Some(rule), None) = (halves.next(), halves.next(), halves.next()) else {
        return false;
    };
    [pack, rule].iter().all(|part| {
        !part.is_empty()
            && part.len() <= MAX_ID_SEGMENT
            && part.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && part.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')
            })
    })
}

/// Longest window a share may aggregate over. A year of daily counts starts to describe
/// a person's working rhythm; two weeks describes a rule's usefulness, which is the point.
pub const MAX_PERIOD_DAYS: u32 = 30;

/// How often a rule fired, for the whole team's benefit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleStat {
    /// Pack-scoped rule id (`tight-scope:no-new-deps`).
    #[serde(rename = "ruleId")]
    pub rule_id: String,
    /// Times the rule was flagged in the period.
    pub flagged: u32,
}

/// Exactly what a share would upload. Serialized and shown to the user first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharePayload {
    /// Packs the user selected. Config only.
    pub packs: Vec<Pack>,
    /// Per-rule counts, pack-scoped ids only.
    #[serde(rename = "ruleStats")]
    pub rule_stats: Vec<RuleStat>,
    /// Days aggregated. Coarse on purpose — no finer than a whole period.
    #[serde(rename = "periodDays")]
    pub period_days: u32,
    /// What was dropped by the filter, and why. Surfaced rather than swallowed: a user
    /// who can see "4 local rules withheld" understands the boundary; one who sees a
    /// silently short list assumes the feature is broken.
    pub withheld: Withheld,
}

/// A count of what the filter refused to share.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Withheld {
    /// Rules whose ids were inferred from the user's own messages.
    #[serde(rename = "localRules")]
    pub local_rules: u32,
    /// Non-constraint signals (saturation, goal drift, degradation). These have no
    /// shareable identity at all — they describe a conversation, not a rule.
    #[serde(rename = "nonRuleSignals")]
    pub non_rule_signals: u32,
}

impl Withheld {
    pub fn any(&self) -> bool {
        self.local_rules > 0 || self.non_rule_signals > 0
    }

    /// A sentence for the panel, naming the boundary rather than apologising for it.
    pub fn explain(&self) -> Option<String> {
        if !self.any() {
            return None;
        }
        let mut parts = Vec::new();
        if self.local_rules > 0 {
            parts.push(format!(
                "{} rule(s) you stated in conversation (sharing the id would reveal what \
                 you said)",
                self.local_rules
            ));
        }
        if self.non_rule_signals > 0 {
            parts.push(format!(
                "{} non-rule signal(s) — saturation and drift describe a conversation, \
                 not a shared rule",
                self.non_rule_signals
            ));
        }
        Some(format!("Not shared: {}.", parts.join("; ")))
    }
}

/// Build the share payload from local flag counts.
///
/// `counts` is `(signal, constraint_id, n)` exactly as
/// `drifterr_store::Store::flag_counts_since` returns it. Filtering happens here, once, so
/// there is a single place to audit.
pub fn build(
    packs: Vec<Pack>,
    counts: &[(String, Option<String>, usize)],
    period_days: u32,
) -> SharePayload {
    let mut rule_stats: Vec<RuleStat> = Vec::new();
    let mut withheld = Withheld::default();

    for (signal, cid, n) in counts {
        // Only constraint violations name a rule. Saturation, goal drift and degradation
        // describe the conversation itself and have nothing shareable in them.
        if signal != "constraint" {
            withheld.non_rule_signals += 1;
            continue;
        }
        match cid.as_deref() {
            Some(id) if is_pack_scoped(id) => rule_stats.push(RuleStat {
                rule_id: id.to_string(),
                flagged: (*n).min(u32::MAX as usize) as u32,
            }),
            // Either no id at all, or a session-local one (`c1`). Both are withheld.
            _ => withheld.local_rules += 1,
        }
    }

    // Deterministic order so two members' payloads diff cleanly and a test can pin it.
    rule_stats.sort_by(|a, b| b.flagged.cmp(&a.flagged).then(a.rule_id.cmp(&b.rule_id)));

    SharePayload {
        packs,
        rule_stats,
        period_days: period_days.min(MAX_PERIOD_DAYS),
        withheld,
    }
}

/// Independent audit of a built payload: does its serialized form contain any of
/// `forbidden`?
///
/// This is belt-and-braces on top of [`build`], and it earns its place because the two
/// fail differently. `build` is a filter over the fields it knows about; this is a search
/// for a string over everything that would actually be transmitted, including anything a
/// future field addition drags along. The caller passes the canaries it cares about — in
/// practice the session's spans and goal — so a regression is caught at the point of
/// egress rather than in review.
pub fn contains_none_of(payload: &SharePayload, forbidden: &[&str]) -> Result<(), String> {
    let json = serde_json::to_string(payload).unwrap_or_default();
    for needle in forbidden {
        let needle = needle.trim();
        // Very short fragments would match by coincidence and make this useless.
        if needle.len() < 4 {
            continue;
        }
        if json.contains(needle) {
            return Err(format!(
                "refusing to share: the payload contains '{needle}', which came from \
                 conversation content"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use drifterr_engine::pack::{PackRule, PACK_VERSION};

    fn pack() -> Pack {
        Pack {
            version: PACK_VERSION,
            name: "Tight scope".into(),
            description: None,
            rules: vec![PackRule {
                id: "no-new-deps".into(),
                text: "No new dependencies".into(),
                why: None,
            }],
        }
    }

    #[test]
    fn pack_scoped_rule_counts_are_shared() {
        let counts = vec![
            (
                "constraint".to_string(),
                Some("tight-scope:no-new-deps".to_string()),
                7,
            ),
            (
                "constraint".to_string(),
                Some("typescript-strict:no-any".to_string()),
                2,
            ),
        ];
        let p = build(vec![pack()], &counts, 14);
        assert_eq!(p.rule_stats.len(), 2);
        // Sorted by count, so the panel leads with the rule that matters most.
        assert_eq!(p.rule_stats[0].rule_id, "tight-scope:no-new-deps");
        assert_eq!(p.rule_stats[0].flagged, 7);
        assert!(!p.withheld.any());
    }

    #[test]
    fn session_inferred_rule_ids_are_withheld_and_counted() {
        // `c1` names a constraint mined from the user's own messages. Publishing the id
        // with a count reveals that they said something the engine turned into a rule.
        let counts = vec![
            ("constraint".to_string(), Some("c1".to_string()), 5),
            ("constraint".to_string(), None, 3),
            (
                "constraint".to_string(),
                Some("tight-scope:no-new-deps".to_string()),
                1,
            ),
        ];
        let p = build(vec![], &counts, 14);
        assert_eq!(
            p.rule_stats.len(),
            1,
            "only the pack rule: {:?}",
            p.rule_stats
        );
        assert_eq!(p.withheld.local_rules, 2);
        // And the user is told, rather than shown a mysteriously short list.
        let why = p.withheld.explain().unwrap();
        assert!(
            why.contains("2 rule(s) you stated in conversation"),
            "{why}"
        );
    }

    #[test]
    fn non_constraint_signals_are_never_shared() {
        // Saturation and goal drift describe a conversation, not a rule. There is no
        // id that could be shared without describing what someone is working on.
        let counts = vec![
            ("saturation".to_string(), None, 4),
            ("goal_alignment".to_string(), None, 2),
            ("degradation".to_string(), None, 1),
        ];
        let p = build(vec![], &counts, 14);
        assert!(p.rule_stats.is_empty());
        assert_eq!(p.withheld.non_rule_signals, 3);
        assert!(p.withheld.explain().unwrap().contains("non-rule signal"));
    }

    #[test]
    fn an_id_that_merely_looks_pack_scoped_is_still_checked() {
        for bad in [
            "c1:",             // trailing colon, empty rule
            ":no-any",         // no pack
            "pack:rule:extra", // not two segments
            "Pack:Rule",       // uppercase — ids are slugs
            "pack rule:x",     // whitespace
            "../etc:passwd",   // path-ish
        ] {
            let counts = vec![("constraint".to_string(), Some(bad.to_string()), 1)];
            let p = build(vec![], &counts, 14);
            assert!(
                p.rule_stats.is_empty(),
                "'{bad}' must not be treated as a pack rule id"
            );
            assert_eq!(p.withheld.local_rules, 1);
        }
    }

    #[test]
    fn the_period_is_capped() {
        let p = build(vec![], &[], 3650);
        assert_eq!(
            p.period_days, MAX_PERIOD_DAYS,
            "a year of counts starts describing a working rhythm"
        );
    }

    #[test]
    fn the_audit_refuses_a_payload_carrying_conversation_content() {
        // The realistic regression: someone adds a field that drags a span along.
        let mut p = build(vec![pack()], &[], 14);
        p.packs[0].description = Some("seen in auth.js: const KEY = 'CANARY-do-not-leak'".into());
        let err = contains_none_of(&p, &["CANARY-do-not-leak"]).unwrap_err();
        assert!(err.contains("refusing to share"), "{err}");
    }

    #[test]
    fn the_audit_passes_a_clean_payload_and_ignores_tiny_fragments() {
        let counts = vec![(
            "constraint".to_string(),
            Some("tight-scope:no-new-deps".to_string()),
            1,
        )];
        let p = build(vec![pack()], &counts, 14);
        assert!(contains_none_of(&p, &["Refactor the auth module", "a", "no"]).is_ok());
    }

    #[test]
    fn the_serialized_payload_holds_only_config_and_counts() {
        // A structural check on the wire format itself: the top-level keys are the four
        // documented ones and nothing has crept in.
        let p = build(vec![pack()], &[], 14);
        let v: serde_json::Value = serde_json::to_value(&p).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["packs", "periodDays", "ruleStats", "withheld"]);
    }
}
