//! On-demand download of the semantic embedding model.
//!
//! # Why this lives here and not in `crates/embeddings`
//!
//! `crates/embeddings` is on the CI-enforced list of crates that may hold chat
//! content and therefore **must have zero network dependencies** (see
//! `crates/proxy/tests/egress.rs`). Adding an HTTP client there would fail that test,
//! and rightly so. Nor does it belong in the proxy or the judge, whose invariant is
//! that they only ever talk to the model provider the user configured.
//!
//! So the fetch lives in the desktop shell, which already does network work for
//! updates and is where app resource management belongs. The embeddings crate keeps a
//! single pure entry point — "load a model from this directory" — and never learns
//! that a network exists.
//!
//! # Why on demand rather than bundled
//!
//! The model is ~127 MB and was bundled by default. That is a large fraction of the
//! install, paid by every user on every update, to feed the signal that fires least
//! often — and since the goal signal's thresholds became scale-free, the lexical
//! embedder detects the eval set's goal-drift case on its own. Bundling it was the
//! most expensive default in the product for the least certain gain.
//!
//! Now: nothing ships, detection runs on the lexical embedder, and a user who wants
//! semantic similarity asks for it once.
//!
//! # Supply chain
//!
//! This downloads a file that an inference runtime will then execute. That is a real
//! attack surface, so the download is **verified against a pinned SHA-256** and
//! discarded on mismatch. An unverified model is not installed, ever — we fall back to
//! lexical instead, which is a working configuration rather than a broken one.

use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::Manager;

/// Where the model files come from. A pre-exported ONNX bge-small-en-v1.5, so no
/// Python toolchain is needed to produce it.
const BASE: &str = "https://huggingface.co/Xenova/bge-small-en-v1.5/resolve/main";

/// Pinned digests for the two files we fetch.
///
/// `None` means "not yet pinned": the download is then refused rather than trusted,
/// because an unpinned model is exactly the supply-chain hole this exists to close.
/// Fill these in from `sha256sum` of the reviewed files, and treat changing them as a
/// deliberate, reviewed act.
const MODEL_SHA256: Option<&str> = None;
const TOKENIZER_SHA256: Option<&str> = None;

/// Refuse anything implausibly large, so a redirect to something unexpected cannot
/// fill the user's disk. bge-small's ONNX export is ~130 MB.
const MAX_MODEL_BYTES: u64 = 200 * 1024 * 1024;
const MAX_TOKENIZER_BYTES: u64 = 8 * 1024 * 1024;

/// How the semantic model is currently provisioned, for the settings view.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    /// Was this build compiled with the ONNX embedder at all?
    pub supported: bool,
    /// Is a usable model present on disk right now?
    pub ready: bool,
    /// Where it came from: "bundled" | "downloaded" | "custom" | "absent".
    pub source: &'static str,
    /// Approximate download size, so the offer can state the cost up front.
    pub download_mb: u32,
    /// True when a download cannot be verified because no digest is pinned.
    pub unpinned: bool,
}

/// The app-data directory the downloaded model lives in.
pub fn download_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|d| d.join("models").join("embed"))
}

/// Does `dir` hold a usable model pair?
pub fn dir_ready(dir: &Path) -> bool {
    dir.join("model.onnx").is_file() && dir.join("tokenizer.json").is_file()
}

/// Report how the semantic model is provisioned.
pub fn status(app: &tauri::AppHandle) -> ModelStatus {
    let supported = cfg!(feature = "semantic");
    // A user-set DRIFTERR_EMBED_MODEL wins and is reported as custom, so the settings
    // view never claims credit for a path the user chose.
    let custom = std::env::var_os("DRIFTERR_EMBED_MODEL")
        .map(PathBuf::from)
        .filter(|p| dir_ready(p));
    let bundled = app
        .path()
        .resource_dir()
        .ok()
        .map(|r| r.join("models").join("embed"))
        .filter(|p| dir_ready(p));
    let downloaded = download_dir(app).filter(|p| dir_ready(p));

    let (ready, source) = if custom.is_some() {
        (true, "custom")
    } else if bundled.is_some() {
        (true, "bundled")
    } else if downloaded.is_some() {
        (true, "downloaded")
    } else {
        (false, "absent")
    };

    ModelStatus {
        supported,
        ready,
        source,
        download_mb: 127,
        unpinned: MODEL_SHA256.is_none() || TOKENIZER_SHA256.is_none(),
    }
}

/// Point `DRIFTERR_EMBED_MODEL` at whichever model is available, preferring a
/// bundled resource over a downloaded one. No-op when the user set it themselves, or
/// when nothing is present — in which case detection runs on the lexical embedder,
/// which is a working configuration, not a degraded one.
pub fn activate_if_present(app: &tauri::AppHandle) {
    if std::env::var_os("DRIFTERR_EMBED_MODEL").is_some() {
        return;
    }
    let bundled = app
        .path()
        .resource_dir()
        .ok()
        .map(|r| r.join("models").join("embed"));
    let candidates = [bundled, download_dir(app)];
    for dir in candidates.into_iter().flatten() {
        if dir_ready(&dir) {
            std::env::set_var("DRIFTERR_EMBED_MODEL", &dir);
            return;
        }
    }
}

/// Download and verify the model into the app-data directory.
///
/// Returns the directory on success. On any failure — network, size cap, digest
/// mismatch — nothing is installed and the app keeps using the lexical embedder.
pub async fn download(app: tauri::AppHandle) -> Result<PathBuf, String> {
    if MODEL_SHA256.is_none() || TOKENIZER_SHA256.is_none() {
        // Deliberate refusal. Downloading an unverified binary that an inference
        // runtime will execute is worse than not having the feature.
        return Err(
            "This build has no pinned model checksum, so the download cannot be \
             verified and is refused. Point DRIFTERR_EMBED_MODEL at a model you \
             obtained yourself, or use the bundled build."
                .to_string(),
        );
    }
    let dir = download_dir(&app).ok_or("cannot resolve the app data directory")?;
    if dir_ready(&dir) {
        return Ok(dir);
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;

    // Download to a temporary name and rename only after verification, so an
    // interrupted or corrupted fetch can never leave a half file that `dir_ready`
    // would then report as usable.
    fetch_verified(
        &client,
        &format!("{BASE}/tokenizer.json"),
        &dir.join("tokenizer.json"),
        TOKENIZER_SHA256,
        MAX_TOKENIZER_BYTES,
    )
    .await?;
    fetch_verified(
        &client,
        &format!("{BASE}/onnx/model.onnx"),
        &dir.join("model.onnx"),
        MODEL_SHA256,
        MAX_MODEL_BYTES,
    )
    .await?;

    Ok(dir)
}

async fn fetch_verified(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    expect_sha: Option<&str>,
    max_bytes: u64,
) -> Result<(), String> {
    let res = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("download failed: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("download failed: HTTP {}", res.status()));
    }
    if let Some(len) = res.content_length() {
        if len > max_bytes {
            return Err(format!(
                "refusing a {len}-byte download (cap {max_bytes}) — unexpected content"
            ));
        }
    }
    let bytes = res
        .bytes()
        .await
        .map_err(|e| format!("download failed: {e}"))?;
    if bytes.len() as u64 > max_bytes {
        return Err("refusing an oversized download — unexpected content".to_string());
    }

    let got = sha256_hex(&bytes);
    match expect_sha {
        Some(want) if got.eq_ignore_ascii_case(want) => {}
        Some(want) => {
            return Err(format!(
                "checksum mismatch for {url}\n  expected {want}\n  got      {got}\n\
                 Nothing was installed."
            ));
        }
        None => return Err("no pinned checksum — refusing to install".to_string()),
    }

    let tmp = dest.with_extension("part");
    std::fs::write(&tmp, &bytes).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, dest).map_err(|e| format!("cannot finalize {}: {e}", dest.display()))?;
    Ok(())
}

/// Minimal SHA-256, so verification needs no extra dependency.
///
/// Written out rather than pulled in because this runs once per install and the
/// alternative is another crate in the supply chain of the thing whose supply chain we
/// are trying to verify.
fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v[7] = v[6];
            v[6] = v[5];
            v[5] = v[4];
            v[4] = v[3].wrapping_add(t1);
            v[3] = v[2];
            v[2] = v[1];
            v[1] = v[0];
            v[0] = t1.wrapping_add(t2);
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(v[i]);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // Spans a block boundary (55/56/64 bytes are the padding edge cases).
        assert_eq!(
            sha256_hex(&[b'a'; 56]),
            "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a"
        );
        assert_eq!(
            sha256_hex(b"The quick brown fox jumps over the lazy dog"),
            "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592"
        );
    }

    #[test]
    fn dir_is_only_ready_with_both_files() {
        let base = std::env::temp_dir().join(format!("drifterr-model-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        assert!(!dir_ready(&base), "empty dir is not ready");
        std::fs::write(base.join("model.onnx"), b"x").unwrap();
        assert!(!dir_ready(&base), "model without a tokenizer is not usable");
        std::fs::write(base.join("tokenizer.json"), b"{}").unwrap();
        assert!(dir_ready(&base));
        let _ = std::fs::remove_dir_all(&base);
    }
}
