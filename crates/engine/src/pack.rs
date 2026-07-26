//! Rule packs — the portable, cross-tool constraint layer.
//!
//! # Why this is the product, not a feature
//!
//! Everything else Drifterr does is scoped to a session. A pack is the one artefact that
//! **composes over time**: rules you've settled on, in a file you own, that you can move
//! between projects, share with a teammate, and keep when you change tools. That makes
//! it the honest competitor to `CLAUDE.md` / `AGENTS.md` / `.cursor/rules` — each of
//! which is per-tool and per-repo, and none of which is checked.
//!
//! Two directions matter equally:
//!
//! * **In** — a pack becomes session constraints, so a new session starts anchored.
//! * **Out** — a pack renders back into a rules file, so the *agent* is told the rules
//!   too, not just the watcher. Detecting a violation is worth less than preventing it,
//!   and the cheapest prevention is writing the rule where the agent already reads.
//!
//! # Packs carry intent, not compiled rules
//!
//! A pack stores each rule as the **natural-language text the user wrote**, never a
//! serialized regex. The check is re-derived on load via [`crate::infer`]. Three
//! consequences, all deliberate:
//!
//! 1. Packs stay human-readable and reviewable — you can diff one in a pull request.
//! 2. A pack written today keeps improving as inference improves; a pack of frozen
//!    regexes would rot.
//! 3. A pack round-trips through a rules file without loss, because a rules file *is*
//!    natural language.
//!
//! The cost is that a rule whose text the engine cannot check yet imports as advisory
//! rather than enforced. That is the right trade: it is visible, and it never
//! manufactures a check nobody asked for.

use crate::baseline::{Checkable, Constraint, ConstraintType};
use crate::infer;
use serde::{Deserialize, Serialize};

/// Current pack schema version. Bump only for a breaking change.
pub const PACK_VERSION: u32 = 1;

/// Upper bound on rules in one pack. A pack is meant to be reviewed by a human before it
/// governs their work; thousands of rules could not be.
pub const MAX_RULES: usize = 200;

/// One rule in a pack: the user's own words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackRule {
    /// Stable, human-meaningful id (`no-any`, `protect-migrations`). Used for the
    /// constraint id, so a violation names something recognisable.
    pub id: String,
    /// The rule as stated. This is the source of truth — the check is inferred from it.
    pub text: String,
    /// Optional note explaining *why*, carried for humans and never parsed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// A portable set of rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pack {
    /// Schema version, so an old reader can refuse a newer pack instead of guessing.
    #[serde(rename = "drifterrPack")]
    pub version: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub rules: Vec<PackRule>,
}

/// What happened when a pack was turned into constraints.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    /// Constraints the engine can check deterministically.
    pub enforced: Vec<Constraint>,
    /// Rule ids the engine could not turn into a check. Reported rather than dropped
    /// silently: a user who thinks a rule is being enforced when it isn't is worse off
    /// than one who knows it's advisory.
    pub advisory: Vec<String>,
}

impl Pack {
    /// Parse a pack, rejecting anything malformed or oversized.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let pack: Pack = serde_json::from_str(text).map_err(|e| format!("invalid pack: {e}"))?;
        pack.validate()?;
        Ok(pack)
    }

    pub fn to_json(&self) -> String {
        // Pretty-printed: a pack is meant to be read, edited and diffed by hand.
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    fn validate(&self) -> Result<(), String> {
        if self.version > PACK_VERSION {
            return Err(format!(
                "pack schema v{} is newer than this build understands (v{PACK_VERSION}) — \
                 upgrade Drifterr rather than risk misreading the rules",
                self.version
            ));
        }
        if self.name.trim().is_empty() {
            return Err("pack has no name".into());
        }
        if self.rules.len() > MAX_RULES {
            return Err(format!(
                "pack has {} rules (max {MAX_RULES}) — a pack should be reviewable",
                self.rules.len()
            ));
        }
        for r in &self.rules {
            if r.id.trim().is_empty() || r.text.trim().is_empty() {
                return Err("every rule needs a non-empty id and text".into());
            }
        }
        Ok(())
    }

    /// Turn the pack into constraints, reporting which rules could not be checked.
    ///
    /// Ids are namespaced (`pack-id:rule-id`) so a violation is traceable to the pack it
    /// came from, and so two packs can hold a rule with the same local id.
    pub fn apply(&self, pack_id: &str) -> Applied {
        let mut enforced = Vec::new();
        let mut advisory = Vec::new();
        for r in &self.rules {
            match infer::infer_rule(&r.text) {
                Some(rule) => {
                    let (_, kind) = infer::describe(&rule);
                    // Skip a rule whose inferred check duplicates one already added —
                    // two phrasings of "no any" are one check.
                    if enforced
                        .iter()
                        .any(|c: &Constraint| c.rule.as_ref() == Some(&rule))
                    {
                        continue;
                    }
                    enforced.push(Constraint {
                        id: format!("{pack_id}:{}", r.id),
                        text: r.text.clone(),
                        kind,
                        checkable: Checkable::Deterministic,
                        active: true,
                        rule: Some(rule),
                    });
                }
                // Not checkable *yet*. Kept as a judge-checkable constraint so the rule
                // still exists in the anchor and shows up in a re-anchor, but it can
                // never drive RED.
                None => {
                    advisory.push(r.id.clone());
                    enforced.push(Constraint {
                        id: format!("{pack_id}:{}", r.id),
                        text: r.text.clone(),
                        kind: ConstraintType::Other,
                        checkable: Checkable::Judge,
                        active: true,
                        rule: None,
                    });
                }
            }
        }
        Applied { enforced, advisory }
    }

    /// Build a pack from recurring rules the user has actually accepted.
    ///
    /// This is what turns standing orders from a per-install curiosity into something
    /// portable: the rules you keep repeating become a file you can carry to the next
    /// project or hand to a teammate.
    pub fn from_texts(name: &str, texts: &[String]) -> Self {
        let mut rules = Vec::new();
        for (i, text) in texts.iter().enumerate() {
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            if rules.len() >= MAX_RULES {
                break;
            }
            rules.push(PackRule {
                id: slug(text, i + 1),
                text: text.to_string(),
                why: None,
            });
        }
        Pack {
            version: PACK_VERSION,
            name: name.to_string(),
            description: Some("Exported from Drifterr standing orders.".into()),
            rules,
        }
    }

    /// Render the pack as a markdown block suitable for appending to `CLAUDE.md`,
    /// `AGENTS.md` or `.cursor/rules`.
    ///
    /// This is the "out" direction, and the more valuable one. Drifterr watching for a
    /// violation is worth less than the agent being told the rule in the first place, and
    /// a rules file is where the agent already looks. The block is fenced by stable
    /// markers so it can be rewritten in place without touching anything a human wrote —
    /// see [`splice_into_rules_file`].
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str(MARKER_BEGIN);
        out.push('\n');
        out.push_str(&format!("\n## {}\n\n", self.name.trim()));
        if let Some(d) = self.description.as_deref().map(str::trim) {
            if !d.is_empty() {
                out.push_str(&format!("{d}\n\n"));
            }
        }
        for r in &self.rules {
            out.push_str(&format!("- {}", r.text.trim()));
            if let Some(w) = r.why.as_deref().map(str::trim) {
                if !w.is_empty() {
                    out.push_str(&format!(" — {w}"));
                }
            }
            out.push('\n');
        }
        out.push('\n');
        out.push_str(MARKER_END);
        out.push('\n');
        out
    }
}

/// Markers delimiting Drifterr's managed block in a rules file.
///
/// Everything between them is ours to rewrite; everything outside is the user's and must
/// survive untouched. Silently reformatting somebody's `CLAUDE.md` would be a good way to
/// never be trusted with it again.
pub const MARKER_BEGIN: &str = "<!-- drifterr:begin (managed — edit in Drifterr) -->";
pub const MARKER_END: &str = "<!-- drifterr:end -->";

/// Insert or replace the managed block in a rules file's contents.
///
/// Idempotent: applying the same pack twice yields identical output, so this can run on
/// every change without churning the file.
pub fn splice_into_rules_file(existing: &str, pack: &Pack) -> String {
    let block = pack.to_markdown();
    match (existing.find(MARKER_BEGIN), existing.find(MARKER_END)) {
        (Some(start), Some(end)) if end > start => {
            let after = end + MARKER_END.len();
            let mut out = String::with_capacity(existing.len() + block.len());
            out.push_str(&existing[..start]);
            out.push_str(block.trim_end());
            out.push_str(&existing[after..]);
            out
        }
        // No managed block yet: append, keeping the user's content first and intact.
        _ => {
            let mut out = existing.trim_end().to_string();
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(&block);
            out
        }
    }
}

/// A short, stable id from a rule's text.
fn slug(text: &str, n: usize) -> String {
    let mut s = String::new();
    let mut last_dash = true;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            s.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && s.len() < 28 {
            s.push('-');
            last_dash = true;
        }
        if s.len() >= 28 {
            break;
        }
    }
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        format!("rule-{n}")
    } else {
        s
    }
}

/// Packs shipped with Drifterr.
///
/// Deliberately few and deliberately boring. Every rule here is one the engine can
/// actually check — a curated pack full of unenforceable aspirations would be exactly the
/// overclaiming this project keeps having to undo.
pub fn builtin() -> Vec<(&'static str, Pack)> {
    let mk = |name: &str, desc: &str, rules: &[(&str, &str)]| Pack {
        version: PACK_VERSION,
        name: name.to_string(),
        description: Some(desc.to_string()),
        rules: rules
            .iter()
            .map(|(id, text)| PackRule {
                id: (*id).to_string(),
                text: (*text).to_string(),
                why: None,
            })
            .collect(),
    };
    vec![
        (
            "typescript-strict",
            mk(
                "TypeScript strict",
                "Keep an agent from loosening types or leaving debug code behind.",
                &[
                    ("no-any", "Never use `any` types"),
                    ("no-console", "No console.log in committed code"),
                    ("no-js", "TypeScript only, no JS files"),
                    ("no-todo", "Never leave TODOs or FIXMEs"),
                ],
            ),
        ),
        (
            "tight-scope",
            mk(
                "Tight scope",
                "Stop an agent widening the blast radius of a small change.",
                &[
                    ("no-new-deps", "No new dependencies"),
                    (
                        "no-new-files",
                        "No new files — work within the existing files",
                    ),
                    ("protect-lockfile", "Don't touch package-lock.json"),
                    ("protect-migrations", "Don't touch the migrations"),
                ],
            ),
        ),
        (
            "security-basics",
            mk(
                "Security basics",
                "The two mistakes worth failing a review over.",
                &[
                    ("no-secrets", "No hardcoded secrets"),
                    ("no-eval", "No eval"),
                ],
            ),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(rules: &[(&str, &str)]) -> Pack {
        Pack {
            version: PACK_VERSION,
            name: "Test pack".into(),
            description: None,
            rules: rules
                .iter()
                .map(|(id, text)| PackRule {
                    id: (*id).into(),
                    text: (*text).into(),
                    why: None,
                })
                .collect(),
        }
    }

    #[test]
    fn json_round_trips() {
        let p = pack(&[("no-any", "Never use `any` types")]);
        let back = Pack::from_json(&p.to_json()).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn a_newer_schema_is_refused_rather_than_guessed_at() {
        let json = r#"{"drifterrPack":99,"name":"Future","rules":[]}"#;
        let err = Pack::from_json(json).unwrap_err();
        assert!(err.contains("newer than this build"), "{err}");
    }

    #[test]
    fn malformed_packs_are_rejected() {
        assert!(Pack::from_json("not json").is_err());
        assert!(Pack::from_json(r#"{"drifterrPack":1,"name":"","rules":[]}"#).is_err());
        // A rule with no text cannot be checked or shown.
        assert!(Pack::from_json(
            r#"{"drifterrPack":1,"name":"x","rules":[{"id":"a","text":" "}]}"#
        )
        .is_err());
        // Oversized: a pack nobody could review.
        let many: Vec<String> = (0..MAX_RULES + 1)
            .map(|i| format!(r#"{{"id":"r{i}","text":"No console.log"}}"#))
            .collect();
        let json = format!(
            r#"{{"drifterrPack":1,"name":"huge","rules":[{}]}}"#,
            many.join(",")
        );
        assert!(Pack::from_json(&json).is_err());
    }

    #[test]
    fn apply_namespaces_ids_and_infers_checks() {
        let applied = pack(&[("no-any", "Never use `any` types")]).apply("typescript-strict");
        assert_eq!(applied.enforced.len(), 1);
        assert_eq!(applied.enforced[0].id, "typescript-strict:no-any");
        assert_eq!(applied.enforced[0].checkable, Checkable::Deterministic);
        assert!(applied.enforced[0].rule.is_some());
        assert!(applied.advisory.is_empty());
        // The user's own wording survives — a flag should quote what they wrote.
        assert_eq!(applied.enforced[0].text, "Never use `any` types");
    }

    #[test]
    fn uncheckable_rules_are_reported_as_advisory_not_dropped() {
        // A real rule the engine cannot verify. It must still exist in the anchor (so a
        // re-anchor restates it) but must never be able to drive RED.
        let applied = pack(&[("clarity", "Prefer composition over inheritance")]).apply("p");
        assert_eq!(applied.advisory, vec!["clarity"]);
        let c = &applied.enforced[0];
        assert_eq!(
            c.checkable,
            Checkable::Judge,
            "advisory can never drive RED"
        );
        assert!(c.rule.is_none());
    }

    #[test]
    fn duplicate_checks_collapse() {
        let applied = pack(&[
            ("a", "Never use `any` types"),
            ("b", "No `any` types anywhere"),
        ])
        .apply("p");
        assert_eq!(
            applied.enforced.len(),
            1,
            "two phrasings of one rule are one check: {:#?}",
            applied.enforced
        );
    }

    #[test]
    fn from_texts_builds_readable_ids() {
        let p = Pack::from_texts(
            "Mine",
            &[
                "No console.log in committed code".to_string(),
                "  ".to_string(), // skipped
            ],
        );
        assert_eq!(p.rules.len(), 1);
        assert_eq!(p.rules[0].id, "no-console-log-in-committed");
        assert!(p.rules[0].id.len() <= 28);
    }

    #[test]
    fn markdown_lists_every_rule_with_its_reason() {
        let mut p = pack(&[("no-any", "Never use `any` types")]);
        p.rules[0].why = Some("it defeats the point of the type checker".into());
        let md = p.to_markdown();
        assert!(md.contains("## Test pack"));
        assert!(md.contains("- Never use `any` types — it defeats the point"));
        assert!(md.starts_with(MARKER_BEGIN));
        assert!(md.trim_end().ends_with(MARKER_END));
    }

    #[test]
    fn splice_appends_without_disturbing_user_content() {
        let existing = "# My project\n\nSome notes I wrote.\n";
        let out = splice_into_rules_file(existing, &pack(&[("no-any", "Never use `any` types")]));
        assert!(out.starts_with("# My project"), "user content stays first");
        assert!(out.contains("Some notes I wrote."));
        assert!(out.contains("Never use `any` types"));
    }

    #[test]
    fn splice_is_idempotent_and_replaces_only_the_managed_block() {
        let existing = "# Mine\n\nKeep me.\n";
        let p1 = pack(&[("no-any", "Never use `any` types")]);
        let once = splice_into_rules_file(existing, &p1);
        let twice = splice_into_rules_file(&once, &p1);
        assert_eq!(
            once, twice,
            "re-applying the same pack must not churn the file"
        );

        // A different pack replaces the block, and still leaves the user's prose.
        let p2 = pack(&[("no-console", "No console.log in committed code")]);
        let updated = splice_into_rules_file(&once, &p2);
        assert!(
            updated.contains("Keep me."),
            "user content survives a rewrite"
        );
        assert!(updated.contains("No console.log"));
        assert!(
            !updated.contains("Never use `any`"),
            "the old managed block is gone, not duplicated: {updated}"
        );
        assert_eq!(
            updated.matches(MARKER_BEGIN).count(),
            1,
            "exactly one managed block"
        );
    }

    #[test]
    fn splice_survives_a_mangled_marker_pair() {
        // An end marker before a begin marker, or a lone marker, must not corrupt the
        // file — worst case we append a fresh block.
        for existing in [
            "# x\n<!-- drifterr:end -->\nstuff\n",
            "# x\n<!-- drifterr:begin (managed — edit in Drifterr) -->\nunclosed\n",
        ] {
            let out = splice_into_rules_file(existing, &pack(&[("no-any", "No `any` types")]));
            assert!(out.contains("No `any` types"), "rule landed: {out}");
            assert!(out.contains("# x"), "user content kept: {out}");
        }
    }

    #[test]
    fn every_builtin_pack_is_valid_and_fully_enforceable() {
        let packs = builtin();
        assert!(packs.len() >= 3);
        for (id, p) in packs {
            p.validate()
                .unwrap_or_else(|e| panic!("builtin pack {id} is invalid: {e}"));
            // The point of a curated pack is that it works. Shipping aspirations the
            // engine cannot check would be exactly the overclaiming to avoid.
            let applied = p.apply(id);
            assert!(
                applied.advisory.is_empty(),
                "builtin pack {id} has unenforceable rules: {:?}",
                applied.advisory
            );
            assert_eq!(applied.enforced.len(), p.rules.len());
            // And it must survive a JSON round trip, since that is how it ships.
            assert_eq!(Pack::from_json(&p.to_json()).unwrap(), p);
        }
    }

    #[test]
    fn a_pack_round_trips_through_a_rules_file() {
        // The cross-tool claim: export to markdown, re-import with the rules-file parser,
        // and the same checks come back.
        let p = pack(&[
            ("no-any", "Never use `any` types"),
            ("no-console", "No console.log in committed code"),
        ]);
        let md = splice_into_rules_file("", &p);
        let reparsed = crate::rules_file::constraints_from_text(&md, "roundtrip");
        let want: Vec<_> = p
            .apply("p")
            .enforced
            .iter()
            .filter_map(|c| c.rule.clone())
            .collect();
        let got: Vec<_> = reparsed.iter().filter_map(|c| c.rule.clone()).collect();
        for w in &want {
            assert!(
                got.contains(w),
                "rule lost in the markdown round trip: {w:?} (got {got:?})"
            );
        }
    }
}
