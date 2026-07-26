//! The shipped rule packs in `packs/` must stay valid, fully enforceable, and identical
//! to the built-ins the binary carries.
//!
//! Two failure modes this pins:
//!
//! * **Drift between the files and the code.** `packs/*.json` is what people read, fork
//!   and reference from CI (`--pack-file packs/tight-scope.json`); `pack::builtin()` is
//!   what `--pack tight-scope` resolves to. If the two diverge, the same pack name means
//!   two different rule sets depending on how you invoked it.
//! * **A pack quietly becoming advisory.** These are curated, so every rule must be one
//!   the engine can actually check. A shipped pack full of unenforceable aspirations is
//!   the overclaiming this project keeps having to undo — worse than shipping no pack,
//!   because the user believes they are protected.

use drifterr_engine::pack::{builtin, Pack};
use std::path::PathBuf;

fn packs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("packs")
}

#[test]
fn every_shipped_pack_file_matches_its_builtin() {
    let dir = packs_dir();
    for (id, expected) in builtin() {
        let path = dir.join(format!("{id}.json"));
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "builtin pack '{id}' has no exported file at {} ({e}). \
                 Export it, or CI's --pack-file path and the binary's --pack path disagree.",
                path.display()
            )
        });
        let on_disk = Pack::from_json(&text)
            .unwrap_or_else(|e| panic!("packs/{id}.json is not a valid pack: {e}"));
        assert_eq!(
            on_disk, expected,
            "packs/{id}.json has drifted from pack::builtin() — the same pack name would \
             mean two different rule sets"
        );
    }
}

#[test]
fn every_pack_file_in_the_directory_is_valid_and_fully_enforceable() {
    let dir = packs_dir();
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).expect("packs/ must exist") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let id = path.file_stem().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path).unwrap();
        let pack = Pack::from_json(&text)
            .unwrap_or_else(|e| panic!("packs/{id}.json is not a valid pack: {e}"));
        let applied = pack.apply(&id);
        assert!(
            applied.advisory.is_empty(),
            "packs/{id}.json ships rules the engine cannot check: {:?}. \
             A curated pack must not promise a check it does not perform.",
            applied.advisory
        );
        assert_eq!(applied.enforced.len(), pack.rules.len());
        seen += 1;
    }
    assert!(seen >= 3, "expected the shipped packs, found {seen}");
}

#[test]
fn the_carve_out_notice_is_present() {
    // The CC BY carve-out in the root LICENSE names these directories. A per-directory
    // notice is what actually reaches someone who receives one file on its own.
    for dir in ["packs", "fixtures", "eval"] {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join(dir)
            .join("LICENSE");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{dir}/LICENSE is missing ({e})"));
        assert!(
            text.contains("CC BY 4.0"),
            "{dir}/LICENSE must name the licence"
        );
    }
}
