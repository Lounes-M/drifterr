//! Baseline import from a project rules file (`CLAUDE.md`, `AGENTS.md`,
//! `.cursor/rules`, …).
//!
//! # Why
//!
//! A developer working in a repo has *already written down* their standing rules —
//! that is exactly what `CLAUDE.md` is. Asking them to retype those rules into a
//! panel before Drifterr can do anything was the single largest slice of
//! time-to-value on the zero-config path: the tool wanted setup work in exchange
//! for a promise. Reading the file they already maintain removes that trade
//! entirely, and it means Drifterr's anchor is the same document their agent is
//! being told to follow.
//!
//! # Precision over recall, deliberately
//!
//! This is an importer for the **hard** signal, so a wrong import is worse than a
//! missed one: it would manufacture a red alert the user never asked for, on a
//! rule they never set. Two consequences:
//!
//! * We only keep lines that [`crate::infer::infer_rules`] can turn into a
//!   deterministic, machine-checkable rule. Prose the engine cannot verify is
//!   skipped rather than guessed at — it stays available to the judge path.
//! * Fenced code blocks are stripped **before** parsing. A rules file routinely
//!   contains example snippets and shell commands (`cargo test`, a sample with a
//!   `// comment` in it), and treating those as rule statements is the obvious way
//!   to import a rule nobody wrote.
//!
//! The engine stays IO-free: this module takes text. Locating and reading the file
//! is the adapter's job (see `drifterr-adapters`).

use crate::baseline::{Checkable, Constraint};
use crate::infer;

/// Rules files we know how to import, in priority order. The first one present in
/// a project wins — they're alternatives, not layers, and merging two of them
/// would silently double up rules that appear in both.
pub const RULES_FILES: &[&str] = &[
    "CLAUDE.md",
    "AGENTS.md",
    ".cursor/rules",
    ".cursorrules",
    ".windsurfrules",
    ".github/copilot-instructions.md",
];

/// Upper bound on imported constraints. A long rules file would otherwise fill the
/// panel with dozens of checks the user never reviewed; the point is a useful
/// starting anchor they can edit, not an exhaustive transcription.
pub const MAX_IMPORTED: usize = 24;

/// Longest line we'll treat as a rule statement. Past this it's prose, and the
/// constraint text would be unreadable in the panel anyway.
const MAX_LINE_CHARS: usize = 240;

/// Parse a rules file's contents into deterministic constraints.
///
/// `id_prefix` seeds the constraint ids (e.g. `"claude-md"` → `claude-md-1`), so
/// the UI and the store can tell imported constraints from ones the user typed.
/// Returns an empty vec when nothing checkable is found — a perfectly normal
/// outcome for a rules file written entirely in prose.
pub fn constraints_from_text(text: &str, id_prefix: &str) -> Vec<Constraint> {
    let prose = strip_code_blocks(text);
    let mut out: Vec<Constraint> = Vec::new();

    for line in prose.lines() {
        let stmt = statement(line);
        if stmt.is_empty() || stmt.chars().count() > MAX_LINE_CHARS {
            continue;
        }
        for rule in infer::infer_rules(stmt) {
            // A rules file often repeats itself across sections; one check per
            // distinct rule is what the user means.
            if out.iter().any(|c| c.rule.as_ref() == Some(&rule)) {
                continue;
            }
            let (_, kind) = infer::describe(&rule);
            out.push(Constraint {
                id: format!("{id_prefix}-{}", out.len() + 1),
                // Keep the user's own wording: when Drifterr flags a violation it
                // should quote the line they wrote, not our paraphrase of it.
                text: stmt.to_string(),
                kind,
                checkable: Checkable::Deterministic,
                active: true,
                rule: Some(rule),
            });
            if out.len() >= MAX_IMPORTED {
                return out;
            }
        }
    }
    out
}

/// Statements that read like rules but produced no check.
///
/// [`constraints_from_text`] is precision-oriented: prose it cannot verify is silently
/// skipped, which is right for the live engine — a hard signal must not guess. But
/// silence is the wrong answer when a *user* wants to know what they're covered for. A
/// rule sitting in someone's `CLAUDE.md` that Drifterr cannot check is exactly the thing
/// they need told, because otherwise they reasonably assume it is enforced.
///
/// Uses [`infer::has_constraint_cue`], which is deliberately recall-oriented, so a line
/// that merely *sounds* like a rule is reported. Over-reporting here is cheap (the user
/// glances and moves on); under-reporting is a false assurance.
pub fn unchecked_statements(text: &str) -> Vec<String> {
    let prose = strip_code_blocks(text);
    let mut out = Vec::new();
    for line in prose.lines() {
        let stmt = statement(line);
        if stmt.is_empty() || stmt.chars().count() > MAX_LINE_CHARS {
            continue;
        }
        if !infer::has_constraint_cue(stmt) {
            continue;
        }
        if !infer::infer_rules(stmt).is_empty() {
            continue; // checkable — not our problem
        }
        let owned = stmt.to_string();
        if !out.contains(&owned) {
            out.push(owned);
        }
        if out.len() >= MAX_IMPORTED {
            break;
        }
    }
    out
}

/// Reduce one raw line to the sentence it asserts, or `""` if it asserts nothing.
///
/// Strips Markdown list markers, heading hashes, blockquote arrows, checkbox
/// boxes and trailing punctuation so `- **Never** use `any`.` and `Never use
/// `any`` reach the inferencer as the same statement.
fn statement(line: &str) -> &str {
    let mut s = line.trim();

    // Blockquotes and list/heading markers, possibly nested ("> - item").
    loop {
        let before = s;
        s = s.trim_start_matches('>').trim_start();
        s = s.trim_start_matches('#').trim_start();
        if let Some(rest) = s.strip_prefix("- ").or_else(|| s.strip_prefix("* ")) {
            s = rest.trim_start();
        } else if let Some(rest) = s.strip_prefix("+ ") {
            s = rest.trim_start();
        } else {
            // Numbered list: "1. ", "2) ".
            let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
            if digits > 0 && digits <= 3 {
                let rest = &s[digits..];
                if let Some(r) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
                    s = r.trim_start();
                }
            }
        }
        // Task-list checkbox.
        for box_ in ["[ ] ", "[x] ", "[X] "] {
            if let Some(rest) = s.strip_prefix(box_) {
                s = rest.trim_start();
            }
        }
        if s == before {
            break;
        }
    }

    // A table row's leading pipe would otherwise glue onto the first word.
    s = s.trim_start_matches('|').trim();
    // Horizontal rules and empty bullets carry no statement.
    if s.chars().all(|c| !c.is_alphanumeric()) {
        return "";
    }
    s.trim_end_matches(['.', ';', ',']).trim()
}

/// Remove fenced code blocks, keeping the surrounding prose.
///
/// Anything inside a fence is an *example*, not a rule. Without this, a rules file
/// that demonstrates the thing it forbids ("don't do this: `const x: any = 1`")
/// would have that very snippet parsed as a rule statement. Inline code spans are
/// kept, because several inference patterns rely on them (``no `any` ``).
fn strip_code_blocks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        // Both ``` and ~~~ fences, with or without a language tag.
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence {
            out.push_str(line);
        }
        // Keep line numbering aligned whether or not the line was kept, so a
        // future caller can map a constraint back to its source line.
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::baseline::Rule;

    fn texts(cs: &[Constraint]) -> Vec<&str> {
        cs.iter().map(|c| c.text.as_str()).collect()
    }

    #[test]
    fn imports_checkable_rules_from_a_markdown_bullet_list() {
        let md = "\
# Project rules

## Code style
- Never use `any` — everything is typed.
- No console.log in committed code.
* No TODOs left behind
1. Keep functions under 40 lines
";
        let cs = constraints_from_text(md, "claude-md");
        assert_eq!(cs.len(), 4, "one constraint per checkable bullet: {cs:#?}");
        assert!(cs.iter().all(|c| c.checkable == Checkable::Deterministic));
        assert!(cs.iter().all(|c| c.active));
        assert!(cs.iter().all(|c| c.rule.is_some()));
        // Ids are prefixed so imported constraints are distinguishable.
        assert_eq!(cs[0].id, "claude-md-1");
        assert_eq!(cs[3].id, "claude-md-4");
        // The user's own wording survives, minus the list marker and full stop.
        assert_eq!(
            texts(&cs)[1],
            "No console.log in committed code",
            "keeps the author's phrasing"
        );
        assert!(matches!(cs[3].rule, Some(Rule::MaxLines { max: 40 })));
    }

    #[test]
    fn ignores_prose_it_cannot_check() {
        // Real, reasonable rules that no deterministic check covers. They must be
        // skipped, not approximated — a hard signal may not guess.
        let md = "\
- Prefer composition over inheritance.
- Write code that reads like the surrounding code.
- Be thoughtful about error handling.
- The architecture should stay channel-agnostic.
";
        assert!(constraints_from_text(md, "x").is_empty());
    }

    #[test]
    fn code_examples_never_become_rules() {
        // The classic failure: a rules file demonstrating what it forbids. The
        // fenced snippet must not be read as a statement.
        let md = "\
Style guide.

```ts
// this comment is an example, not a rule
const x: any = 1;
console.log(x);
```

Only the line below is an actual rule.
- No new dependencies without discussion
";
        let cs = constraints_from_text(md, "x");
        assert_eq!(cs.len(), 1, "only the real rule is imported: {cs:#?}");
        assert_eq!(texts(&cs)[0], "No new dependencies without discussion");
    }

    #[test]
    fn tilde_fences_are_stripped_too() {
        let md = "~~~js\nconsole.log('x') // no comments here please\n~~~\n";
        assert!(constraints_from_text(md, "x").is_empty());
    }

    #[test]
    fn duplicate_rules_collapse() {
        let md = "\
- No `any` types
## Repeated later in the file
- Avoid `any` types anywhere
";
        let cs = constraints_from_text(md, "x");
        assert_eq!(cs.len(), 1, "the same rule stated twice is one constraint");
    }

    #[test]
    fn strips_markdown_decoration() {
        for line in [
            "- No TODOs.",
            "* No TODOs",
            "+ No TODOs",
            "1. No TODOs",
            "2) No TODOs",
            "> - No TODOs",
            "- [ ] No TODOs",
            "### No TODOs",
            "|  No TODOs",
        ] {
            let cs = constraints_from_text(line, "x");
            assert_eq!(cs.len(), 1, "should parse a rule out of {line:?}");
            assert_eq!(cs[0].text, "No TODOs", "clean text from {line:?}");
        }
    }

    #[test]
    fn respects_the_import_cap() {
        // Many distinct checkable rules; the cap keeps the panel reviewable.
        let mut md = String::new();
        for n in 1..=40 {
            md.push_str(&format!("- Keep functions under {} lines\n", n + 5));
        }
        let cs = constraints_from_text(&md, "x");
        assert!(
            cs.len() <= MAX_IMPORTED,
            "imported {} constraints, cap is {MAX_IMPORTED}",
            cs.len()
        );
    }

    #[test]
    fn skips_overlong_lines() {
        let long = format!("- No TODOs {}", "x".repeat(MAX_LINE_CHARS));
        assert!(constraints_from_text(&long, "x").is_empty());
    }

    #[test]
    fn empty_and_decoration_only_input_is_safe() {
        for input in ["", "\n\n", "---", "# Rules", "| --- | --- |", "```\n```"] {
            assert!(
                constraints_from_text(input, "x").is_empty(),
                "unexpected constraints from {input:?}"
            );
        }
    }

    #[test]
    fn french_rules_import_too() {
        let md = "- Pas de commentaires dans le code\n- Pas de `any`\n";
        let cs = constraints_from_text(md, "x");
        assert_eq!(cs.len(), 2, "EN/FR parity: {cs:#?}");
    }

    #[test]
    fn known_rules_files_are_listed_in_priority_order() {
        assert_eq!(RULES_FILES[0], "CLAUDE.md");
        assert!(RULES_FILES.contains(&"AGENTS.md"));
        assert!(RULES_FILES.contains(&".cursor/rules"));
    }
}
