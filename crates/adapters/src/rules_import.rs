//! Locate a project's rules file and import its checkable rules.
//!
//! The parsing lives in the engine ([`drifterr_engine::rules_file`]); this module
//! is the IO half — finding the file, reading it, and walking up from a working
//! directory to the repository root the way the agent tools themselves do.
//!
//! This is what makes the zero-config path genuinely zero-config: a developer with
//! a `CLAUDE.md` already has their standing rules written down, so Drifterr starts
//! with a real anchor instead of an empty form.

use drifterr_engine::baseline::Constraint;
use drifterr_engine::rules_file::{self, RULES_FILES};
use std::path::{Path, PathBuf};

/// How far up the tree to look for a rules file. Enough to get from a nested
/// crate or package directory to the repo root, without wandering into `$HOME`
/// and importing rules from an unrelated project.
const MAX_ASCENT: usize = 6;

/// A rules file found on disk, with the constraints it yielded.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    /// The file the rules came from, for display ("Imported from CLAUDE.md").
    pub path: PathBuf,
    /// Deterministic constraints parsed from it. May be empty when the file is
    /// entirely prose — a normal outcome, not an error.
    pub constraints: Vec<Constraint>,
}

/// Find the nearest rules file at or above `start` and import it.
///
/// Walks up at most [`MAX_ASCENT`] directories, taking the first file present in
/// [`RULES_FILES`] priority order at each level — nearest directory wins, so a
/// package-level `CLAUDE.md` beats the repo root's. Returns `None` when no rules
/// file exists anywhere on the path, or when it cannot be read.
pub fn discover(start: &Path) -> Option<Imported> {
    let mut dir = Some(start);
    for _ in 0..=MAX_ASCENT {
        let d = dir?;
        for name in RULES_FILES {
            let path = d.join(name);
            // `read_to_string` fails on directories and on non-UTF-8 files; both are
            // "not a rules file we can use", so just keep looking.
            if let Ok(text) = std::fs::read_to_string(&path) {
                return Some(Imported {
                    constraints: rules_file::constraints_from_text(&text, &id_prefix(name)),
                    path,
                });
            }
        }
        dir = d.parent();
    }
    None
}

/// A stable, readable id prefix derived from the file name: `CLAUDE.md` →
/// `claude-md`, `.cursor/rules` → `cursor-rules`. Keeps imported constraint ids
/// self-describing in the panel and in the store.
fn id_prefix(file_name: &str) -> String {
    let mut out = String::with_capacity(file_name.len());
    let mut last_dash = true; // suppress a leading dash from ".cursorrules"
    for ch in file_name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A scratch directory that cleans itself up. Avoids a dev-dependency for
    /// three tests' worth of temp-dir handling.
    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let mut p = std::env::temp_dir();
            p.push(format!(
                "drifterr-rules-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn write(&self, rel: &str, body: &str) -> PathBuf {
            let path = self.0.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, body).unwrap();
            path
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn imports_claude_md_from_the_starting_directory() {
        let t = Tmp::new("direct");
        let expected = t.write("CLAUDE.md", "# Rules\n- Never use `any`\n- No TODOs\n");
        let got = discover(&t.0).expect("should find CLAUDE.md");
        assert_eq!(got.path, expected);
        assert_eq!(got.constraints.len(), 2);
        assert_eq!(got.constraints[0].id, "claude-md-1");
    }

    #[test]
    fn walks_up_to_the_repo_root() {
        let t = Tmp::new("ascend");
        t.write("CLAUDE.md", "- No console.log\n");
        let nested = t.0.join("crates/engine/src");
        fs::create_dir_all(&nested).unwrap();
        let got = discover(&nested).expect("should ascend to the root CLAUDE.md");
        assert_eq!(got.constraints.len(), 1);
    }

    #[test]
    fn nearest_directory_wins_over_the_root() {
        let t = Tmp::new("nearest");
        t.write("CLAUDE.md", "- No TODOs\n");
        let pkg = t.0.join("packages/web");
        t.write("packages/web/CLAUDE.md", "- No console.log\n- No TODOs\n");
        let got = discover(&pkg).expect("should find the package-level file");
        assert_eq!(got.path, pkg.join("CLAUDE.md"));
        assert_eq!(got.constraints.len(), 2, "the closer file's rules are used");
    }

    #[test]
    fn claude_md_beats_other_names_in_the_same_directory() {
        let t = Tmp::new("priority");
        t.write("AGENTS.md", "- No TODOs\n");
        let claude = t.write("CLAUDE.md", "- No console.log\n");
        assert_eq!(discover(&t.0).unwrap().path, claude);
    }

    #[test]
    fn finds_cursor_rules_too() {
        let t = Tmp::new("cursor");
        let expected = t.write(".cursor/rules", "- Never use `any`\n");
        let got = discover(&t.0).expect("should find .cursor/rules");
        assert_eq!(got.path, expected);
        assert_eq!(got.constraints[0].id, "cursor-rules-1");
    }

    #[test]
    fn a_prose_only_rules_file_imports_cleanly_with_nothing() {
        // Present but unparseable into checks: still a successful discovery, just
        // with no constraints. The caller must not treat this as an error.
        let t = Tmp::new("prose");
        t.write("CLAUDE.md", "# Style\nPrefer clarity over cleverness.\n");
        let got = discover(&t.0).expect("file exists, so it's found");
        assert!(got.constraints.is_empty());
    }

    #[test]
    fn no_rules_file_anywhere_is_none() {
        let t = Tmp::new("empty");
        let nested = t.0.join("a/b");
        fs::create_dir_all(&nested).unwrap();
        assert!(discover(&nested).is_none());
    }

    #[test]
    fn id_prefixes_are_readable() {
        assert_eq!(id_prefix("CLAUDE.md"), "claude-md");
        assert_eq!(id_prefix("AGENTS.md"), "agents-md");
        assert_eq!(id_prefix(".cursor/rules"), "cursor-rules");
        assert_eq!(id_prefix(".cursorrules"), "cursorrules");
        assert_eq!(
            id_prefix(".github/copilot-instructions.md"),
            "github-copilot-instructions-md"
        );
    }
}
