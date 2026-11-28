//! Startup step 3: build the embedder `--embedder-backend` names. Moved
//! verbatim out of `async_main`.

use std::sync::Arc;

use primer_core::embedder::Embedder;
use primer_engine::{
    build_fastembed_embedder, build_ollama_embedder, build_openai_compat_embedder,
};

use crate::Cli;

/// The embedder for hybrid retrieval, or `None` for BM25-only. Exits the
/// process on an unknown kind or a build-feature error, as before.
pub(crate) async fn build_embedder(cli: &Cli) -> Option<Arc<dyn Embedder>> {
    // ─── Embedder ────────────────────────────────────────────────────
    //
    // `none` (the default) skips embedder construction entirely so the
    // dialogue manager runs BM25-only retrieval — the pre-Phase-0.2.5
    // behaviour. `stub` constructs a deterministic hash embedder useful
    // only for testing the hybrid pipeline; with no semantic signal it
    // dilutes BM25 with noise, so it is not the production default.
    // `fastembed` and `ollama` need their respective cargo features;
    // if missing, the dispatch helpers return Err and the CLI exits
    // with the message (the GUI surfaces the same Err inline).
    //
    // Real-backend construction failures fall back to BM25-only with a
    // tracing warn — the conversation still works, which is strictly
    // better than refusing to start.
    match cli.embedder_backend.as_str() {
        "none" => None,
        "stub" => Some(Arc::new(primer_embedding::StubEmbedder::new()) as _),
        "fastembed" => match build_fastembed_embedder(cli.embedder_model.as_deref()) {
            Ok(opt) => opt,
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        },
        "ollama" => match build_ollama_embedder(
            cli.embedder_ollama_url.as_deref(),
            cli.embedder_model.as_deref(),
        )
        .await
        {
            Ok(opt) => opt,
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        },
        "openai-compat" => {
            let url = cli
                .embedder_openai_compat_url
                .as_deref()
                .or(Some(cli.openai_compat_url.as_str()));
            match build_openai_compat_embedder(
                url,
                cli.embedder_openai_compat_model
                    .as_deref()
                    .or(cli.embedder_model.as_deref()),
                cli.openai_compat_api_key.clone(),
            )
            .await
            {
                Ok(opt) => opt,
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
        }
        other => {
            eprintln!(
                "Error: unknown --embedder-backend {other:?}; expected one of none, stub, fastembed, ollama, openai-compat"
            );
            std::process::exit(1);
        }
    }
}
