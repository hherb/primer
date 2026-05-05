# Hybrid Retrieval (BM25 + Vector) — Design Spec

**Date:** 2026-05-05
**Phase:** Phase 0.2.5 (between KB bootstrapping and the Python ingestion pipeline)
**Author:** Claude (with Horst Herb)
**Status:** Approved by user, implementation starting

## Decisions from review

- **Default model: BGE-M3** (multilingual, 1024-dim, ~570 MB int8 via fastembed-rs). Locked because the initial test cohort — three of Horst's grandchildren — is bi/trilingual: two native German+English (one with basic Japanese), one native English with basic Hindi and German. EN + DE must work from day one without a model swap on the `--language` flag, and a smaller English-only default would have meant the German-speaking children hit a 570 MB download as soon as they switched languages — exactly the wrong first impression. BGE-M3 covers all four of those languages in one model.
- **Test matrix: English + German minimum.** The existing English seed corpus (55 passages) gets vectors first; a German seed corpus (drafted later, separate cluster batch) is the second locale. Japanese and Hindi are stretch — BGE-M3 supports them so the runtime story is the same; what's missing is the seed content.
- **On-insert embedding cadence:** sync for KB ingestion (offline by nature; one-shot work), `tokio::spawn` for session turns (mirrors classifier/extractor/comprehension flow; runs in the inter-turn pause).
- **Embedder construction site:** `primer-cli` constructs one `Arc<dyn Embedder>` at startup and shares it across the dialogue manager (retrieval + on-insert) and the kb-load tool (ingestion). Same pattern as `Arc<dyn InferenceBackend>`.
- **Model-mismatch on session DBs:** hard error, same as the KB. A learner who switches embedding models has to explicitly re-embed via `primer-storage-reembed --force`. Silent fallback would invisibly degrade long-term-memory recall — exactly the kind of subtle regression that erodes trust in the system.
- **`fastembed` feature default-on.** Keyword-only retrieval errors are grating for children (this is the whole motivation for hybrid search), so the default build path delivers the actual hybrid experience. New users hit a 570 MB first-run download; we mitigate with (a) a clear one-line banner explaining the one-time setup, (b) `--embedder-backend stub` as the escape hatch for offline / no-network users, and (c) `StubEmbedder` as the runtime fallback if the download fails (hybrid degrades to BM25-only with a `tracing::warn!`, never blocks the conversation).

## Goal

Replace the BM25-only retrieval path in both `primer-knowledge` and `primer-storage` with a hybrid path that fuses BM25 lexical matching with dense-vector semantic similarity. The motivation is concrete: the seed-corpus retrieval-quality test exposed three queries (out of 56) where the BM25 path could not surface the semantically correct passage even at `top_k=5`, because filler tokens like "how", "does", "grow", "travel" got out-voted in the OR-joined query. At 50 K Wikipedia passages the failure rate would be far worse, and the same failure mode is even more damaging in the long-term-memory path — paraphrase ("my sister" → stored "Mariam") is the conversational norm and BM25 cannot bridge it.

After this work:

- A new **`primer-embedding`** crate exposes an `Embedder` trait and three implementations: `StubEmbedder` (deterministic, in-memory, for tests), `FastEmbedBackend` (default; wraps `fastembed-rs` over ONNX Runtime), and `OllamaEmbedder` (prototyping; `/api/embeddings`).
- Default model: **BGE-M3 dense** (multilingual, 1024-dim, ~570 MB int8-quantised via fastembed). One model serves every locale we currently or plausibly support.
- `primer-knowledge` schema v3 adds an `embeddings_<pack>` table (one row per passage, BLOB column for the float vector). `primer-storage` schema v8 adds an `embeddings_turns` table keyed by `(session_id, turn_index)`.
- New `KnowledgeBase::retrieve_hybrid(query, &dyn Embedder, params)` and `SessionStore::retrieve_session_turns_hybrid(...)` perform RRF fusion of a BM25 top-K and a vector top-K.
- The dialogue manager's two retrieval helpers (`retrieve_knowledge`, `retrieve_long_term_memory`) switch to the hybrid methods when an embedder is wired and fall back to the BM25-only methods otherwise.
- New CLI flags `--embedder-backend stub|fastembed|ollama`, `--embedder-model <name>`, `--embedder-timeout-ms <ms>` mirror the classifier/extractor/comprehension pattern. Default: `stub` so default builds remain hermetic; `fastembed` is opt-in via a feature flag.
- A `primer-kb-load --reembed` mode backfills embeddings for an existing corpus DB, so the existing seed corpus picks up vectors when the user opts in.
- The existing BM25-only `retrieve` / `retrieve_session_turns` stay intact — hybrid layers on top, never replaces.
- The retrieval-quality test gains a hybrid variant that uses `StubEmbedder` (structural assertions on RRF behaviour) plus an `#[ignore]`-by-default real-model variant for local manual runs.

## Non-goals

- **No removal of BM25.** Lexical matching is still the right answer for rare, high-information terms ("photosynthesis", "Jupiter"). Hybrid combines the two; we do not switch to vector-only.
- **No cross-encoder re-ranking.** A separate re-ranker would improve top-1 quality further but at much higher cost (bi-encoder + cross-encoder per query, ~ms). RRF over bi-encoder + BM25 is the v1.
- **No multi-vector / sparse BGE-M3 modes.** BGE-M3 supports three modes (dense, sparse, multi-vector ColBERT-style). We use only the **dense** vector. Sparse adds complexity for marginal gain alongside FTS5; multi-vector blows up storage.
- **No on-the-fly embedding of every existing passage.** v1 embeds passages on insert (when an embedder is configured) and provides an explicit `--reembed` tool to backfill. We do not embed at retrieval time except for the query.
- **No `sqlite-vec` extension.** v1 stores embeddings as `BLOB`s and does cosine scan in Rust. At ≤50 K rows this scans in ~30 ms. `sqlite-vec` is a future optimisation when corpus size justifies it; the trait method signatures will not change.
- **No per-locale embedding model.** BGE-M3 is multilingual. Per-locale models are a future possibility but not v1.
- **No quantised vector storage** (8-bit, binary). Stored as `f32`. Storage cost (1024 × 4 = 4 KB per row) is acceptable for the corpus sizes we plan.
- **No background re-embedding on schema bumps**. If we later swap models, a `--reembed --force` is the explicit path. Mixed-model corpora are rejected by an `embedding_model` lookup table check at open-time, mirroring how the storage crate validates lookup-table drift.
- **No automatic model download at `cargo build` time.** First-run download into `~/.primer/models/` is on-demand at backend construction. Documented.
- **No changes to `Passage` / `Turn` public types.** Vectors are an internal storage detail; consumers see `Vec<Passage>` exactly as today.
- **No re-architecting of `RetrievalParams`.** A new `HybridParams` struct extends it with vector-side knobs (`vector_top_k`, `rrf_k`); `retrieve_hybrid` takes both.
- **No HNSW / IVF index in v1.** A flat scan with cosine over ≤50 K rows is fast enough. At 500 K we reconsider, with the trait API stable.

## Pedagogical principles preserved

- **All learner data stays local.** Embeddings of session turns are stored in the same per-child DB as the turns themselves. The embedder runs on-device for `fastembed`; the cloud `OllamaEmbedder` is for developer-prototyping only and is not the recommended runtime.
- **The Primer asks more questions than it answers.** Better retrieval gives the LLM richer specifics to ask follow-up questions about; it does not push toward a more answer-y stance.
- **Comprehension is verified through transfer questions.** Hybrid retrieval over long-term memory means the Primer can pull up a child's *paraphrased* earlier statement and check transfer against it, where today's FTS5-only path could not.
- **Offline-first.** Hybrid path requires the embedder model on disk; it does not require network at runtime.

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│  primer-cli                                                         │
│  • constructs one Embedder (Arc) at startup                         │
│  • passes it into DialogueManager AND into the KB ingestion path    │
└──────────────┬──────────────────────────────────────────────────────┘
               │ Arc<dyn Embedder>
               ▼
┌──────────────────────────┐    ┌─────────────────────────────────┐
│  primer-embedding (NEW)  │    │  primer-pedagogy/dialogue_mgr   │
│  trait Embedder          │    │  retrieve_knowledge():          │
│    embed(&[&str])        │    │    if embedder.is_some():       │
│    dim() -> usize        │    │      kb.retrieve_hybrid(...)    │
│    name() -> &str        │    │    else:                        │
│  StubEmbedder            │    │      kb.retrieve(...)           │
│  FastEmbedBackend        │    │  retrieve_long_term_memory():   │
│  OllamaEmbedder          │    │    same hybrid/fallback shape   │
└──────┬───────────────────┘    └────────────┬────────────────────┘
       │                                     │
       ▼                                     ▼
┌─────────────────────────┐         ┌────────────────────────────┐
│  primer-knowledge       │         │  primer-storage            │
│  schema v3:             │         │  schema v8:                │
│    embeddings_<pack>    │         │    embeddings_turns        │
│      (rowid, vec BLOB)  │         │      (session_id, turn_idx,│
│  retrieve_hybrid(       │         │       vec BLOB)            │
│    query, embedder,     │         │  retrieve_session_turns_   │
│    HybridParams)        │         │    hybrid(...)             │
│  insert_passage_with_   │         │  save_turn_embedding(...)  │
│    embedding(...)       │         │  embedding_model lookup    │
│  embedding_model lookup │         │    table                   │
│    table                │         │                            │
└─────────────────────────┘         └────────────────────────────┘
```

### The `Embedder` trait (`primer-core::embedder`)

```rust
#[async_trait]
pub trait Embedder: Named + Send + Sync {
    /// Embed a batch of texts. Returns one vector per input, all of `dim()` length.
    /// Implementations should batch under the hood for throughput.
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;

    /// Vector dimensionality. Constant for the lifetime of the embedder.
    fn dim(&self) -> usize;

    /// A model identifier persisted in the DB so we can detect cross-model
    /// mixing (e.g. someone re-opens a DB with a different embedder).
    fn model_id(&self) -> &str;
}
```

`Named` lives in `primer-core`; `Embedder` joins it alongside the speech-trait family. The trait sits in `primer-core::embedder` so neither `primer-knowledge` nor `primer-storage` needs to depend on `primer-embedding` at compile time — the same dep-graph hygiene as for `Inference`.

`Result<Vec<Vec<f32>>>` (rather than `Result<ndarray::Array2<f32>>`) keeps `primer-core` free of any heavy numeric dep. `f32` (not `f64`) — embedding-model output is `f32` natively and storage cost is what matters here.

### The `primer-embedding` crate

Three implementations:

#### `StubEmbedder`
Deterministic. Hashes each input string into a fixed-dim vector via FxHash → splat → L2-normalise. Two identical strings produce identical vectors; vectors are fully reproducible across processes. This is what the workspace tests use, the same way `StubBackend` is the default in `primer-inference`. Default `dim = 384` (matches bge-small if anyone wants to swap to it; bge-m3 is 1024).

#### `FastEmbedBackend`
Wraps `fastembed-rs` (Qdrant's crate; uses `ort` 2.x, which we already pin). Constructed with a model identifier (`bge-m3`, `bge-small-en-v1.5`, `multilingual-e5-small`, etc. — fastembed's enum). On first construction, fastembed downloads the int8-quantised ONNX model from Hugging Face into `~/.cache/fastembed/` (or a path overridable via env var). Subsequent runs load from cache. Default model: **`BGEM3`** (1024-dim, ~570 MB int8).

This crate gates its `fastembed-rs` dep behind a feature flag `fastembed` so default builds (and CI) don't pull the dep tree. Pattern matches `primer-speech`'s approach: stubs always available, real backends behind features.

```toml
[features]
default = []
fastembed = ["dep:fastembed"]
ollama = []  # only needs reqwest, which we already have transitively
```

#### `OllamaEmbedder`
For prototyping. Hits `http://localhost:11434/api/embeddings` with `{model: "...", prompt: "..."}`. `nomic-embed-text` is the typical Ollama default (768-dim). Useful for laptops where pulling a fastembed model is awkward; not the recommended production path.

### Storage: schema additions

#### `primer-knowledge` schema v3

```sql
-- One row per passage. content_rowid keeps it 1:1 with the FTS table.
CREATE TABLE IF NOT EXISTS embeddings_<pack>(
    content_rowid INTEGER PRIMARY KEY
        REFERENCES passages_<pack>_content(rowid) ON DELETE CASCADE,
    model_id      INTEGER NOT NULL REFERENCES embedding_models(id),
    vec           BLOB NOT NULL  -- f32 little-endian, dim floats
);
CREATE INDEX IF NOT EXISTS ix_embeddings_<pack>_model
    ON embeddings_<pack>(model_id);

-- Cross-locale lookup table for which embedding models have been used
-- in this DB. Validates against the embedder's reported model_id at open
-- time; drift is a hard error. (Same pattern as primer-storage's lookup
-- tables.)
CREATE TABLE IF NOT EXISTS embedding_models(
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    dim  INTEGER NOT NULL
);
```

The `embeddings_<pack>` table is per-locale (because the FTS table is); the `embedding_models` lookup is shared. A passage that was inserted before the embedder was configured has no row in `embeddings_<pack>` — `retrieve_hybrid` simply returns those passages only via the BM25 leg.

#### `primer-storage` schema v8

```sql
CREATE TABLE IF NOT EXISTS embeddings_turns(
    turn_id   INTEGER PRIMARY KEY REFERENCES turns(id) ON DELETE CASCADE,
    model_id  INTEGER NOT NULL REFERENCES embedding_models(id),
    vec       BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_embeddings_turns_session_model
    ON embeddings_turns(model_id);

CREATE TABLE IF NOT EXISTS embedding_models(
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    dim  INTEGER NOT NULL
);
```

Same lookup-table policy. Both crates carry their own `embedding_models` table; consolidating into a shared crate is unnecessary duplication-discipline (the model registry is per-DB anyway).

### Hybrid retrieval: Reciprocal Rank Fusion

```rust
pub struct HybridParams {
    pub bm25_top_k: usize,        // default 20
    pub vector_top_k: usize,      // default 20
    pub final_top_k: usize,       // default 5 (returned to caller)
    pub rrf_k: f64,               // default 60.0 (industry standard)
    pub min_score: f64,           // post-fusion floor; default 0.0
    pub source_filter: Vec<String>,
}
```

```rust
async fn retrieve_hybrid(
    &self,
    query: &str,
    embedder: &dyn Embedder,
    params: &HybridParams,
) -> Result<Vec<Passage>> {
    // 1. BM25 leg: existing path, top bm25_top_k.
    let bm25 = self.retrieve(query, &RetrievalParams {
        top_k: params.bm25_top_k, ..Default::default()
    }).await?;

    // 2. Vector leg: embed query, scan stored vectors, top vector_top_k by cosine.
    let q_vec = embedder.embed(&[query]).await?.into_iter().next().unwrap();
    let vec_hits = self.knn_scan(&q_vec, params.vector_top_k)?;

    // 3. RRF fuse.
    let mut scores: HashMap<String, f64> = HashMap::new();
    for (rank, p) in bm25.iter().enumerate() {
        *scores.entry(p.id.clone()).or_default()
            += 1.0 / (params.rrf_k + rank as f64 + 1.0);
    }
    for (rank, p) in vec_hits.iter().enumerate() {
        *scores.entry(p.id.clone()).or_default()
            += 1.0 / (params.rrf_k + rank as f64 + 1.0);
    }

    // 4. Sort, threshold, take final_top_k.
    // ...
}
```

`knn_scan` reads every embedding row, computes cosine, sorts, takes top-K. Pure Rust, no extension. At 50 K rows × 1024 floats this is ~50 ms single-threaded; rayon-parallelised it's ~10 ms. We start single-threaded and revisit if it shows up in profiling.

The same fusion lives in `primer-storage::retrieve_session_turns_hybrid`. Identical algorithm, different storage layout (turns are scoped per-session, embeddings are scoped per-turn).

### Where embedding happens

**On insert** (preferred path):
- `SqliteKnowledgeBase` gains an optional `embedder: Option<Arc<dyn Embedder>>` set via `with_embedder(&mut self, ...)` after construction. When set, `insert_passage` embeds and writes both rows in one transaction.
- `SqliteSessionStore` likewise. The dialogue manager already owns the storage; the embedder is plumbed in via `DialogueManager::new(..., embedder: Option<Arc<dyn Embedder>>)`, mirroring the classifier/extractor/comprehension pattern.

**On reindex** (one-shot):
- `primer-kb-load --reembed --knowledge-db <path> --locale <pack>` walks every passage missing an embedding (or every passage if `--force`) and writes vectors. Used to backfill an existing seed-corpus DB after a user opts into hybrid search.
- A symmetric `primer-storage-reembed` binary for session DBs, behind a `--reembed-sessions` flag. Lower priority — session DBs grow turn-by-turn, so on-insert embedding handles new content.

**On retrieve** (every call):
- The query alone is embedded once per call. ~50–100 ms for bge-m3 on CPU. This sits comfortably inside the inter-turn latency budget.

### CLI plumbing

New flags in `primer-cli` (mirroring the classifier/extractor/comprehension pattern):

```
--embedder-backend stub|fastembed|ollama        (default: stub)
--embedder-model <name>                         (default: bge-m3 for fastembed,
                                                 nomic-embed-text for ollama)
--embedder-timeout-ms <ms>                      (default: 5000)
--embedder-cache-dir <path>                     (default: $XDG_CACHE_HOME/primer/models)
```

`--embedder-backend stub` keeps default builds hermetic and CI fast — same rule as the rest of the LLM-adjacent backends. `fastembed` is unlocked only when `cargo run --features primer-cli/embedding` is in effect, which transitively enables `primer-embedding/fastembed`. The `embedding` feature is separate from `speech` even though both pull `ort`, because users wanting a text-mode Primer with vector retrieval shouldn't be forced into the heavier speech build.

### Locale + cross-DB consistency

- A KB DB carries an `embedding_models` row identifying which model was used. On open, if `--embedder-backend` is configured, the CLI verifies the embedder's `model_id()` matches one of the rows. Mismatch → hard error with a clear message ("DB embedded with `bge-m3` (dim 1024); current backend is `bge-small-en-v1.5` (dim 384). Re-embed with `--reembed --force` or pass `--embedder-model bge-m3`.").
- A session DB carries the same check. Switching embedding models for an existing learner requires `primer-storage-reembed --force`.
- BGE-M3 being multilingual means the `--language` flag never forces a model swap. This is the main reason it's the default.

### Determinism, tests, CI

- `StubEmbedder` is fully deterministic, dim-configurable, and has a 4-line implementation. Every workspace test that touches retrieval can use it.
- `cargo test --workspace` continues to run with default features (no fastembed dep, no model download). CI stays green at exactly 491+N tests.
- A new gated test `--features primer-embedding/fastembed --test hybrid_retrieval_real_model` runs the actual model against the seed corpus. Marked `#[ignore]` so `cargo test` doesn't run it without `--ignored`. Exists for local sanity-checks, not CI.
- The retrieval-quality test gets a hybrid variant using `StubEmbedder`. The structural assertion is "hybrid output never has fewer relevant hits than BM25 alone" (`for q in QUERIES: hybrid_top_k(q) ⊇ at-least-one-of bm25_top_k(q) AND hybrid_top_k(q) is also non-empty`). The full-quality assertion runs only under the gated test.

## Implementation order

1. **Spec review** (this doc). Land after Horst's review.
2. **`primer-core::embedder` trait + `Named` integration.** ~30 lines. Tests: trait-shape sanity.
3. **`primer-embedding` crate with `StubEmbedder` only.** Default-feature build, no fastembed dep. ~80 lines + tests for determinism, batching, dim correctness.
4. **`primer-knowledge` schema v3** (additive, idempotent migration following the v1→v2 pattern). New `insert_passage_with_embedding` + `knn_scan` private helper. `retrieve_hybrid` calls existing `retrieve` for the BM25 leg. Tests: schema version round-trip, stub-driven hybrid retrieval, RRF fusion correctness.
5. **`primer-storage` schema v8** (same shape; additive). Tests as for #4.
6. **Wire `Option<Arc<dyn Embedder>>` into `DialogueManager`.** Two helpers (`retrieve_knowledge`, `retrieve_long_term_memory`) gain the if-some/else branch. Pedagogy tests pass with `None` (today's behaviour) and with `Stub`.
7. **`primer-embedding::FastEmbedBackend`** behind the `fastembed` feature. Dep on `fastembed-rs`. Smoke test against the real model, marked `#[ignore]`.
8. **`primer-embedding::OllamaEmbedder`**. Hits `/api/embeddings`. Smoke test marked `#[ignore]` (requires Ollama running locally).
9. **CLI flags** + per-flag wiring. Reject mismatched embedding models on open. Default: `stub`.
10. **`primer-kb-load --reembed`** mode. Backfills embeddings for the in-repo seed corpus when the user runs it. Verified by running locally with `--features primer-cli/embedding` and re-running the retrieval-quality test against the resulting DB.
11. **CLAUDE.md updates** — the embedder trait, the schema-v3/v8 bumps, the model-mismatch policy, the determinism story.

This sequencing means steps 2–6 land on default features with zero external deps and zero behavioural change for non-opt-in users; steps 7–10 unlock real models. CI stays default-features throughout. The seed corpus gets vectors only when a developer opts in.

## Risks and mitigations

- **Risk: `fastembed-rs`'s ort version lags ours.** We pin `ort = "=2.0.0-rc.10"` for vendored silero/whisper/piper. If fastembed pins a different rc, we get a duplicate-version error or a vendor-patch headache. **Mitigation:** check fastembed-rs's current ort version before step 7 and either match it or vendor the relevant fastembed code, mirroring the existing piper-rs vendor approach. Worst case we patch fastembed's Cargo.toml.
- **Risk: BGE-M3's int8 model is ~570 MB and the first-run download is slow on weak networks.** **Mitigation:** documented in README; `--embedder-cache-dir` lets users pre-stage. A future "slim" build profile could ship `bge-small-en-v1.5` (~33 MB) as default with bge-m3 as opt-in for multilingual.
- **Risk: cosine scan over 500 K rows is too slow.** **Mitigation:** the trait surface stays the same; we swap in `sqlite-vec` (KNN via virtual table) without API change. Until then, ≤50 K rows is the sweet spot. The `passage_count()` already exists; we can warn at open time when crossing the threshold.
- **Risk: model swap on an existing DB silently degrades quality.** **Mitigation:** the `embedding_models` lookup table + open-time validation makes this a hard error, not a silent regression.
- **Risk: stub-only CI fails to catch real-model regressions.** **Mitigation:** the gated `--features fastembed --ignored`-by-default test exists for local validation. A nightly (or weekly) CI job could run it on a self-hosted runner with the model pre-downloaded; deferred until we see the first real-model regression.
- **Risk: embedding the child's turn at every save adds latency.** **Mitigation:** save spawns the embedding in a `tokio::spawn` and writes the vector when ready, mirroring the classifier/extractor flow. The vector is available for the next-turn retrieval; missing it on the just-saved turn is irrelevant because that turn is in the immediate context window already (FTS retrieval skips the window).
- **Risk: privacy — turn embeddings could leak verbatim text under inversion attacks.** **Mitigation:** the embeddings live in the same per-child SQLite file as the turns themselves. They never leave the device. Inversion attacks are a real research topic; the threat is no worse than storing the turns in plaintext, which we already do.
- **Risk: concept extractor + classifier + comprehension + embedder all hitting the same backend (Ollama / cloud) at once causes contention.** **Mitigation:** they already run sequentially per turn (classifier task, then extractor → comprehension chain). Embedding-on-insert runs after `save_session` returns, in the same inter-turn pause. No new contention is introduced; documented in the dialogue-manager flow comment.

## Open questions for review

All five questions settled — see the Decisions from review section above. No outstanding open items.
