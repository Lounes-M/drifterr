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
    let mut out: Vec<Constraint> = Vec::new();

    for cand in statements(text) {
        let stmt = &cand.text;
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
                text: stmt.clone(),
                kind,
                checkable: Checkable::Deterministic,
                active: true,
                // Imported, therefore proposed rather than enforced. See
                // `Constraint::proposed`: an importer reading natural language can
                // never be perfect, so a mistake here must cost an amber proposal
                // the user glances at, not a red alert on a rule nobody wrote.
                proposed: true,
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
    let mut out = Vec::new();
    for cand in statements(text) {
        // A heading is a label for the rules below it far more often than it is a
        // rule. Reporting "Architecture rules (don't break these)" as something
        // Drifterr cannot check is noise in a list a person actually reads.
        if cand.heading {
            continue;
        }
        if !infer::has_constraint_cue(&cand.text) {
            continue;
        }
        if !infer::infer_rules(&cand.text).is_empty() {
            continue; // checkable — not our problem
        }
        if !out.contains(&cand.text) {
            out.push(cand.text);
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

/// Turn a markdown document into the list of candidate rule *statements*.
///
/// # Why this is not `text.lines()`
///
/// It used to be, and that was the bug. Prose in a rules file is hard-wrapped, so
/// a single sentence arrives as several physical lines and each was evaluated as
/// a rule on its own. Drifterr's own `CLAUDE.md` contains the paragraph
///
/// ```text
/// drift scores are never sent there. When adding to the backend, keep that line
/// bright — if it touches chat content, it does not belong in Supabase.
/// ```
///
/// whose first physical line matched "backend" and "keep" and became a hard,
/// RED-capable "server-side only" constraint. Nobody wrote that rule. It then
/// fired on any reply containing `document.getElementById` — in a repository that
/// ships a browser extension and a panel UI.
///
/// Two changes follow from that. Wrapped lines are joined back into the block they
/// belong to, and blocks are split into **sentences**, so a clause is judged with
/// the sentence it lives in rather than with whatever happened to share its line.
///
/// # What counts as a block
///
/// A block ends at a blank line, at the start of the next list item, and at a
/// heading. Headings themselves are dropped: "Architecture rules (don't break
/// these)" is a section label, not a rule, and treating labels as rules is what
/// filled the *unchecked* list with table-of-contents noise.
fn statements(text: &str) -> Vec<Candidate> {
    let prose = strip_code_blocks(text);
    let mut blocks: Vec<Candidate> = Vec::new();
    let mut current = String::new();

    let flush = |cur: &mut String, out: &mut Vec<Candidate>| {
        let t = cur.trim();
        if !t.is_empty() {
            out.push(Candidate {
                text: t.to_string(),
                heading: false,
            });
        }
        cur.clear();
    };

    for raw in prose.lines() {
        let trimmed = raw.trim();

        // A blank line always ends a block.
        if trimmed.is_empty() {
            flush(&mut current, &mut blocks);
            continue;
        }
        // A heading ends the previous block and stands alone — it never joins the
        // paragraph beneath it. It stays a candidate, because a heading is
        // sometimes genuinely the rule ("## Never use `any`"), but it is marked so
        // the recall-oriented `unchecked_statements` can drop it: a section label
        // like "Architecture rules (don't break these)" reported as an unverifiable
        // rule is table-of-contents noise, and that list is read by a human.
        if is_heading(trimmed) {
            flush(&mut current, &mut blocks);
            let text = statement(trimmed);
            if !text.is_empty() {
                blocks.push(Candidate {
                    text: text.to_string(),
                    heading: true,
                });
            }
            continue;
        }
        // A table row is a cell grid, not a sentence; each cell is its own
        // candidate so a one-rule-per-row table still imports.
        if is_table_row(trimmed) {
            flush(&mut current, &mut blocks);
            for cell in trimmed.split('|') {
                let cell = cell.trim();
                if !cell.is_empty() {
                    blocks.push(Candidate {
                        text: cell.to_string(),
                        heading: false,
                    });
                }
            }
            continue;
        }
        // A new list item starts a new block; a continuation line joins the
        // current one. This is the join that makes wrapped prose whole again.
        if starts_list_item(trimmed) {
            flush(&mut current, &mut blocks);
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(trimmed);
    }
    flush(&mut current, &mut blocks);

    blocks
        .iter()
        .flat_map(|b| {
            let heading = b.heading;
            split_sentences(&b.text)
                .into_iter()
                .map(move |s| Candidate {
                    text: statement(&s).to_string(),
                    heading,
                })
        })
        .filter(|c| !c.text.is_empty() && c.text.chars().count() <= MAX_LINE_CHARS)
        .collect()
}

/// One candidate rule statement, plus whether it came from a heading.
struct Candidate {
    text: String,
    /// Headings can be rules, but they are also section labels — see
    /// [`unchecked_statements`], which is the one caller that must tell them apart.
    heading: bool,
}

/// ATX (`## x`) and setext (`---` / `===`) headings.
fn is_heading(line: &str) -> bool {
    let l = line.trim_start_matches('>').trim_start();
    if l.starts_with('#') {
        return true;
    }
    let bare = l.trim();
    !bare.is_empty()
        && (bare.chars().all(|c| c == '=') || (bare.chars().all(|c| c == '-') && bare.len() >= 3))
}

fn is_table_row(line: &str) -> bool {
    let l = line.trim();
    l.starts_with('|') && l.matches('|').count() >= 2
}

/// Does this line begin a new list item (bullet, numbered, or task)?
fn starts_list_item(line: &str) -> bool {
    let l = line.trim_start_matches('>').trim_start();
    if l.starts_with("- ") || l.starts_with("* ") || l.starts_with("+ ") {
        return true;
    }
    let digits = l.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && digits <= 3 {
        let rest = &l[digits..];
        return rest.starts_with(". ") || rest.starts_with(") ");
    }
    false
}

/// Split a block into sentences.
///
/// Deliberately simple — `. `, `! `, `? ` and the end of the block — because the
/// alternative is a sentence tokenizer, and every one of them has opinions about
/// abbreviations that would be wrong here in a different way. The failure mode of
/// splitting too eagerly is a shorter statement, which under-claims; the failure
/// mode of not splitting is the bug this replaced.
///
/// A period inside an inline code span or immediately between word characters
/// (`console.log`, `package.json`, `e.g.`) is not a break, because those are the
/// exact tokens the rules depend on.
fn split_sentences(block: &str) -> Vec<String> {
    let chars: Vec<char> = block.chars().collect();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut in_code = false;
    for i in 0..chars.len() {
        let c = chars[i];
        if c == '`' {
            in_code = !in_code;
            continue;
        }
        if in_code || !matches!(c, '.' | '!' | '?') {
            continue;
        }
        // Must be followed by whitespace (or end) to be a sentence end.
        let next = chars.get(i + 1);
        let ends = match next {
            None => true,
            Some(n) => n.is_whitespace(),
        };
        if !ends {
            continue;
        }
        // `console.log` and `v1.2` keep their dots: a break needs a non-alnum or
        // a whitespace-separated word before it.
        if c == '.' {
            let prev = if i == 0 { None } else { Some(chars[i - 1]) };
            // A single letter before the period is almost always an initialism
            // or an abbreviation ("e.g.", "i.e."), not a sentence end.
            let two_back = if i >= 2 { Some(chars[i - 2]) } else { None };
            if matches!(prev, Some(p) if p.is_alphanumeric())
                && matches!(two_back, Some(t) if t == '.')
            {
                continue;
            }
        }
        let piece: String = chars[start..=i].iter().collect();
        if !piece.trim().is_empty() {
            out.push(piece.trim().to_string());
        }
        start = i + 1;
    }
    let tail: String = chars[start..].iter().collect();
    if !tail.trim().is_empty() {
        out.push(tail.trim().to_string());
    }
    if out.is_empty() {
        out.push(block.to_string());
    }
    out
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

    /// The regression that motivated sentence-level parsing.
    ///
    /// This is a verbatim paragraph from Drifterr's own `CLAUDE.md`. Hard-wrapped,
    /// its first physical line reads "…never sent there. When adding to the
    /// backend, keep that line" — which the old line-based importer turned into a
    /// hard "server-side only" constraint that then fired RED on any reply
    /// containing `document.getElementById`, in a repository that ships a browser
    /// extension and a panel UI.
    ///
    /// Nobody wrote that rule. Importing nothing here is the correct answer.
    #[test]
    fn wrapped_prose_never_becomes_a_rule() {
        let md = "\
- **Local-first.** Conversations live in local SQLite; **no chat content ever
  leaves the machine.** Model calls (judge) go through the user's own provider.
  The one server-side component is **accounts & billing** (Supabase + Stripe,
  see `supabase/` and `docs/ACCOUNTS.md`): it holds identity (email, plan,
  subscription status) and nothing else. Conversations, prompts, signals and
  drift scores are never sent there. When adding to the backend, keep that line
  bright — if it touches chat content, it does not belong in Supabase.
";
        let cs = constraints_from_text(md, "claude-md");
        assert!(
            cs.is_empty(),
            "prose about a backend is not a layer constraint, got: {:#?}",
            texts(&cs)
        );
    }

    /// Wrapped lines are joined before anything is inferred, so a rule that spans
    /// two physical lines still imports as one statement — and a clause is never
    /// judged with whatever happened to share its line.
    #[test]
    fn wrapped_lines_are_joined_into_one_statement() {
        let md = "\
- Never leave a TODO
  in committed code.
";
        let cs = constraints_from_text(md, "x");
        assert_eq!(cs.len(), 1, "{:#?}", texts(&cs));
        assert_eq!(cs[0].text, "Never leave a TODO in committed code");
    }

    /// Two rules in one paragraph are two statements, not one run-on.
    #[test]
    fn a_paragraph_splits_into_sentences() {
        let md = "Never use `any` types. Also keep functions under 30 lines.\n";
        let cs = constraints_from_text(md, "x");
        assert_eq!(cs.len(), 2, "{:#?}", texts(&cs));
        assert!(texts(&cs).iter().any(|t| t.contains("any")));
        assert!(texts(&cs).iter().any(|t| t.contains("30 lines")));
    }

    /// A period inside a token is not a sentence boundary — the tokens the rules
    /// are built from would not survive being split there.
    #[test]
    fn dotted_tokens_do_not_split_sentences() {
        let cs = constraints_from_text("No console.log in committed code.\n", "x");
        assert_eq!(cs.len(), 1, "{:#?}", texts(&cs));
        assert_eq!(cs[0].text, "No console.log in committed code");
    }

    /// Everything imported is a *proposal*. The rule check stays deterministic, but
    /// whether the user asked for the rule was decided by reading English, and a
    /// signal that may drive RED must not rest on that.
    #[test]
    fn imported_constraints_are_proposed_not_enforced() {
        let cs = constraints_from_text("- No console.log in committed code\n", "claude-md");
        assert_eq!(cs.len(), 1);
        assert!(
            cs[0].proposed,
            "an imported rule must start as a proposal, not a red-capable constraint"
        );
    }

    /// Section labels are not rules the user is owed a warning about. This list is
    /// read by a person, and filling it with headings and wrapped fragments is how
    /// it stops being read.
    #[test]
    fn unchecked_statements_skip_headings_and_wrapped_fragments() {
        let md = "\
## Architecture rules (don't break these)

- **The engine is channel-agnostic.** It only ever sees the normalized
  `Conversation`.
";
        let un = unchecked_statements(md);
        assert!(
            !un.iter().any(|s| s.contains("Architecture rules")),
            "a section heading is not an unverifiable rule: {un:#?}"
        );
        assert!(
            !un.iter().any(|s| s.ends_with("normalized")),
            "a wrapped fragment must never be reported as a rule: {un:#?}"
        );
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
