//! Quick embedder probe: prints cosine similarities so you can see what the
//! lexical model catches vs a semantic (ONNX) one. Handy when tuning the soft
//! signals or deciding whether the semantic model is worth bundling.
//!
//!   cargo run -p drifterr-embeddings --example similarity
//!   DRIFTERR_EMBED_MODEL=/path/to/bge cargo run -p drifterr-embeddings \
//!     --features onnx --example similarity

use drifterr_embeddings::{cosine, default_embedder, BagEmbedder, Embedder};

fn main() {
    // The eval's e1 case: a Lambda-latency goal, an on-topic turn, a drift turn.
    let goal = "Reduce cold-start latency of our AWS Lambda functions";
    let ontopic =
        "Enable provisioned concurrency and move heavy imports below the handler so they don't run on every init.";
    let drift =
        "Actually, let's redesign the landing page hero and pick a fresh brand colour palette for the marketing site.";

    let show = |name: &str, e: &dyn Embedder| {
        let g = e.embed(goal);
        println!(
            "{name:<9} goal·ontopic = {:.3}   goal·drift = {:.3}   (drift should be << ontopic)",
            cosine(&g, &e.embed(ontopic)),
            cosine(&g, &e.embed(drift)),
        );
    };

    show("lexical", &BagEmbedder::default());
    // `default_embedder` returns the ONNX model when built with `--features onnx`
    // and DRIFTERR_EMBED_MODEL points at one; otherwise it's the lexical model.
    show("default", default_embedder().as_ref());
}
