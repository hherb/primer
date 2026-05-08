# Knowledge and retrieval

This chapter is about how the Primer grounds its replies in real, attributed text. It covers the [KnowledgeBase](src/crates/primer-core/src/knowledge.rs) trait, the SQLite + FTS5 implementation, the lexical (BM25) and hybrid (BM25 + dense-vector RRF) retrieval paths, the [Embedder](src/crates/primer-core/src/embedder.rs) trait and its three concrete backends, the multi-layer auto-seeded corpus that ships in the repo, and the locale-aware schema that lets every language live side-by-side without contaminating each other's BM25 statistics. Two recipes at the end show how to extend the seed corpus and how to add a brand-new locale.

If you already read chapter 3 on inference and pedagogy, the dialogue manager you saw there pulls retrieved passages into the system prompt before every turn — this chapter is the back half of that pipeline.

## The KnowledgeBase trait

Every retrieval surface the dialogue manager sees is one trait, defined in [knowledge.rs](src/crates/primer-core/src/knowledge.rs). The contract is small on purpose: backends decide how to store and rank passages, but the trait shape is fixed so the engine never knows whether it is talking to a SQLite FTS5 index, a future on-device vector store, or a stub.

```rust
// src/crates/primer-core/src/knowledge.rs
#[async_trait]
pub trait KnowledgeBase: Send + Sync {
    async fn retrieve(&self, query: &str, params: &RetrievalParams) -> Result<Vec<Passage>>;

    async fn retrieve_for_context(
        &self,
        conversation_summary: &str,
        params: &RetrievalParams,
    ) -> Result<Vec<Passage>> { /* default: delegates to retrieve */ }

    async fn upsert_source(&self, _source: &SourceMeta) -> Result<()> { /* default no-op */ }
    async fn list_sources(&self) -> Result<Vec<SourceMeta>> { /* default empty */ }

    async fn retrieve_hybrid(
        &self,
        query: &str,
        embedder: &dyn Embedder,
        params: &HybridParams,
    ) -> Result<Vec<Passage>> { /* default: BM25-only fallback */ }
}
```

A few details worth pinning down before you read further. `Passage` is `{ id, source, text, score }`, where `score` is "higher = more relevant" regardless of the underlying ranker — backends must normalise their internal scoring into that direction before returning. `SourceMeta` is the per-source attribution + licence record (`id`, `license`, `attribution`, `source_url`, `retrieved_at`); a passage's `source` field is a logical foreign key into `SourceMeta::id`. `RetrievalParams` is BM25-shaped (`top_k`, `min_score`, `source_filter`); `HybridParams` adds the wider candidate pool (`bm25_top_k`, `vector_top_k`) plus the RRF constant (`rrf_k`). Both `Default` impls read from [consts::retrieval](src/crates/primer-core/src/consts.rs) so any tuning change happens in one place.

> **Note:** When no `Embedder` is wired, `retrieve_hybrid`'s default trait impl falls back to BM25-only — every consumer can call it unconditionally. This is what keeps the dialogue manager free of `if cfg!(feature = "embedding")` branches.

## SqliteKnowledgeBase schema

The concrete backend is [SqliteKnowledgeBase](src/crates/primer-knowledge/src/lib.rs), which uses SQLite FTS5 as its lexical index and stores per-passage `f32` vectors as BLOBs for the dense leg. It is locale-scoped: every locale gets its own pair of tables plus its own embedding store, so a German corpus and an English corpus can coexist in one file without polluting each other's term statistics.

For locale `<pack_id>` (e.g. `en`, `de`), the per-locale layout is:

- `passages_<pack_id>` — the FTS5 virtual table, used for BM25 ranking.
- `passages_<pack_id>_content` — the external-content storage table that backs the FTS5 index. The plan and some older comments call this `passages_<pack_id>_meta`; the actual table name in source is `_content`.
- `embeddings_<pack_id>` — one row per passage with the `f32` vector stored as a BLOB column, plus a `model_id` foreign key.

Two cross-locale tables travel alongside:

- `sources` — licence + attribution metadata, keyed by source id. Phase 0.2 schema; populated lazily by `upsert_source`.
- `embedding_models` — `(name, dim)` registry for every embedder ever seen on this DB. Mismatched dim under an existing name is a hard error rather than a silent fallback.

The schema is at [USER_VERSION](src/crates/primer-knowledge/src/lib.rs) `3`. Migrations layer additively, each idempotent and transaction-wrapped: v2 added `sources`; v3 added `embedding_models` and the per-locale `embeddings_<pack>` tables. A pre-locale legacy `passages` table (from before the locale-aware refactor) is migrated into the requested locale's `passages_<pack>_content` pair on first open and the legacy table is dropped — the migration assumes the legacy corpus matches the locale being opened, which is the only sound assumption when locale wasn't tracked at write time.

To open a knowledge base, prefer the locale-explicit constructor:

```rust
use primer_core::i18n::Locale;
use primer_knowledge::SqliteKnowledgeBase;

let kb = SqliteKnowledgeBase::open_for_locale(path, Locale::English)?;
```

The shorter `SqliteKnowledgeBase::open(path)` exists for back-compat but is `#[deprecated]` — it silently routes a non-English corpus through the English-default migration path, which is exactly the bug class the deprecation message warns about.

## BM25 ranking

SQLite FTS5 ranks results with BM25, which is the lexical-overlap baseline that has powered most search engines since the 1990s. One quirk worth knowing: FTS5 returns BM25 scores as **negative** numbers (more negative = more relevant), because internally the ranker is sorted ascending. The `KnowledgeBase` public API negates this when it builds a `Passage`, so callers always see "higher = more relevant" — the `score` field is monotonic in the natural sense. If you ever read raw `bm25(...)` values in a debugging session, remember the sign flip when comparing them to what the trait returns.

Child input is sanitised before it touches FTS5. Tokens are stripped of non-alphanumeric characters, FTS5 reserved keywords (`AND`, `OR`, `NOT`, `NEAR`) are dropped, surviving tokens are quoted and OR-joined. An empty result short-circuits the query. This is what allows the long-term-memory layer in `primer-storage` to take raw child input safely; BM25 + `LIMIT k` keeps the OR-join from producing noise.

## Hybrid retrieval

Lexical search is great when the child uses the corpus's own vocabulary and shaky when they don't. Hybrid retrieval addresses the second case by running a second leg over dense vectors — semantic similarity in an embedding space — and fusing both ranked lists.

`SqliteKnowledgeBase::retrieve_hybrid` does exactly that:

1. Run BM25 with `top_k = bm25_top_k` (default 30 — wider than what's ultimately returned, so the fusion stage has a candidate pool to work with).
2. Embed the query through the supplied [Embedder](src/crates/primer-core/src/embedder.rs) and run a cosine search over the per-passage vectors with `top_k = vector_top_k` (default 30).
3. Fuse the two ranked lists via Reciprocal Rank Fusion (`primer_core::rrf::fuse(&[ranked_lists], k)`) with `rrf_k = 60` — the industry-standard default. Smaller values weight the very top of each list more heavily; larger values flatten the curve.
4. Apply `min_score` and return the top `final_top_k` (default 5).

`HybridParams::default()` reads from [consts::retrieval](src/crates/primer-core/src/consts.rs); a `default_mirrors_consts` test pins the alignment so a code reviewer doesn't have to chase two sources of truth.

> **Note:** Bumping the candidate pool sizes (`KB_BM25_TOP_K`, `KB_VECTOR_TOP_K`) is cheap — it only affects in-memory work, not what ends up in the prompt. Bumping `KB_FINAL_TOP_K` is what costs prompt tokens, because every returned passage is injected into the system prompt every turn.

The fallback story is important. When the dialogue manager has no `Embedder` wired (the BM25-only configuration that was the pre-Phase-0.2.5 default), the trait's default `retrieve_hybrid` impl quietly delegates to `retrieve` with `final_top_k` and `min_score`. That means every consumer can call `retrieve_hybrid` unconditionally; a future flag-flip to "hybrid by default" needs no change at the call site.

## The Embedder trait

The dense leg requires a way to turn text into a fixed-dimension `f32` vector. That contract lives in [embedder.rs](src/crates/primer-core/src/embedder.rs):

```rust
// src/crates/primer-core/src/embedder.rs
#[async_trait]
pub trait Embedder: Named + Send + Sync {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    fn dim(&self) -> usize;
    fn model_id(&self) -> &str;
}
```

`Embedder` inherits from `Named`, which is the same single-source-of-truth `name()` trait that all the speech traits inherit from — backends never have to write `name()` twice. `model_id` is a stable identifier (e.g. `"bge-m3"`, `"stub-fxhash-v1"`) persisted alongside every stored vector so cross-model mixing is detectable at open time.

Three concrete impls live in [primer-embedding](src/crates/primer-embedding/src/lib.rs):

### StubEmbedder

Always built, no feature flag. It hashes each input through FNV + xorshift to produce a deterministic, L2-normalised `f32` vector of configurable dimension (default 384). Model id is `"stub-fxhash-v1"`. This is the embedder every workspace test uses when it needs a non-trivial vector backend without a model download. See [stub.rs](src/crates/primer-embedding/src/stub.rs).

> **Gotcha:** `--embedder-backend stub` is for testing the hybrid pipeline structurally only. In production it dilutes BM25 with semantic noise from a hash function — two synonyms produce orthogonal vectors — and should never be the default. If you see suspicious recall behaviour, the first thing to check is which embedder is actually wired.

### FastEmbedBackend

Behind the `fastembed` feature. Wraps [fastembed-rs](https://github.com/Anush008/fastembed-rs) `5.7.0` to load BGE-M3, a 1024-dimensional multilingual embedding model that covers EN/DE/JA/HI in one binary so the initial test cohort never hits a model swap. The int8-quantised weights weigh in at about 570 MB. See [fastembed_backend.rs](src/crates/primer-embedding/src/fastembed_backend.rs).

> **Gotcha:** First-run BGE-M3 download is ~570 MB into `~/.cache/primer/models/` and uses the same `cdn.pyke.io` ONNX-runtime download as the speech feature. Construction failure falls back to BM25-only with a `tracing::warn!` — the conversation still works, which is strictly better than refusing to start.

### OllamaEmbedder

Behind the `ollama` feature. Calls the local Ollama daemon's `/api/embeddings` endpoint (default model `nomic-embed-text`, default URL `http://localhost:11434`). This is the developer-prototyping path — useful for trying out a different embedding model without rebuilding the binary, but not the recommended runtime backend because it adds a network hop in the hot path. See [ollama.rs](src/crates/primer-embedding/src/ollama.rs).

> **Gotcha:** The `embedding` and `speech` cargo features both pin `ort = "=2.0.0-rc.10"`. Bumping fastembed past `5.7.0` will break the speech build until the vendored silero/whisper/piper crates get rc.11+ patches. The two feature stacks share an ort version by load-bearing constraint, not by accident.

> **Why:** Mismatched embedding dim under an existing `model_id` is a hard error in both `primer-knowledge` (schema v3) and `primer-storage` (schema v8). Silent fallback would invisibly degrade retrieval; the loud failure is the single mechanism that prevents that bug class. To deliberately swap models on an existing knowledge-base file, run `primer-kb-load --reembed --force --knowledge-db <path> --locale <pack>`.

## Auto-seed and multi-file discovery

A fresh KB starts empty. Rather than ask every contributor to populate one by hand, the loader at [primer-kb-load](src/crates/primer-kb-load/src/lib.rs) auto-seeds on first open if the database is empty.

`auto_seed_if_empty(kb, locale)` is the entry point — it is called unconditionally during CLI startup. The function checks `kb.passage_count()`; if non-zero it returns `Ok(None)` immediately. If zero, it discovers seed files for the locale and loads each in turn.

Discovery is the interesting part. `discover_seed_files(locale)` walks a fixed precedence list of candidate directories:

1. `$PRIMER_SEED_DIR` — environment override, useful for tests and packaged builds.
2. `$XDG_DATA_HOME/primer/seed/` — the standard cross-distro user data path. Falls back to `~/.local/share/primer/seed/` when `XDG_DATA_HOME` is unset.
3. `$CARGO_MANIFEST_DIR/.../data/seed/` — the in-repo path, walking up from the crate's manifest dir up to five levels. This is what makes `cargo run` from `src/` work without any environment setup.

Whichever directory yields at least one matching file wins, and **all** matching `*.<pack_id>.jsonl` files in that directory are returned, sorted lexicographically and loaded in that order. "Matching" means a regular file whose name ends in `.<pack>.jsonl` — distractors like `wiki_passages.de.jsonl` (wrong locale) and `README.md` (wrong extension) are correctly ignored when the locale is English.

This is what makes the multi-layer corpus work transparently. Today, for `Locale::English`, the in-repo seed dir holds two files:

- `seed_passages.en.jsonl` — 55 hand-drafted passages, CC0-1.0, covering the five planned clusters (space, body, how-things-work, life, earth/weather).
- `wiki_passages.en.jsonl` — 35 lead paragraphs from Simple English Wikipedia, CC-BY-SA-3.0, covering physics fundamentals, chemistry, biology, earth science, and health.

Both load on a fresh English KB, giving 90 passages out of the box. A future `wiki_passages.de.jsonl` would auto-load on a German session without any code change. The older `discover_seed_jsonl(locale)` API still exists for back-compat (it returns just the canonical `seed_passages.<pack>.jsonl`); new code should prefer `discover_seed_files`.

The loader half — `load_jsonl(kb, path)` — is idempotent. It treats `id` as the deduplication key: a row already in `passages_<pack>_content` with the same `id` is skipped, not overwritten, mirroring the append-only behaviour of `SessionStore::save_session`. Re-running on the same JSONL is a no-op; running it on a JSONL with new entries adds only the new ones. The `sources` table uses upsert semantics so attribution stays current.

> **Note:** `auto_seed_if_empty` runs unconditionally on every CLI startup, but the empty-KB check makes the actual load conditional. After the first run the KB is non-empty and the function is a `passage_count` query plus an early return.

## Wikipedia ingestion pipeline

The Wikipedia layer is regenerated by a Python pipeline at [data/ingest/](data/ingest/), not at build time — the developer runs it once and commits the resulting JSONL. This keeps Rust builds hermetic (no network, no Python dep) and makes changes to the corpus reviewable as ordinary diffs.

The design is: pure functions where possible; network is the test boundary, injected via an `http_client` parameter so the unit tests can run with a mocked transport. Source files live next to a `tests/` directory that exercises the pure halves directly.

Two functions do the actual fetching, in [simple_wikipedia.py](data/ingest/simple_wikipedia.py):

- `fetch_lead(title, *, http_client)` — fetch one article's lead paragraph. Raises loudly on missing pages, empty extracts, and disambiguation pages (matched by `_DISAMBIGUATION_PATTERNS` against the lead's first 300 characters; the cure is a more specific title like `Base (chemistry)` rather than `Base`).
- `fetch_leads(titles, *, http_client)` — batched variant that fetches up to 20 titles per API call. Strictly better than calling `fetch_lead` in a loop because the MediaWiki extracts endpoint enforces a per-IP rate limit; one request for twenty leads costs a single rate-limit slot.

The whitelist at [simple_wikipedia_whitelist.txt](data/ingest/simple_wikipedia_whitelist.txt) is hand-curated. To expand it, [build_whitelist.py](data/ingest/build_whitelist.py) pulls candidates from Wikipedia's Vital Articles Level 4 list — they still need manual review because the Vital list includes plenty of articles that aren't appropriate for an eight-year-old.

To regenerate the wiki layer:

```bash
# from the repo root
python3 -m venv .venv
source .venv/bin/activate
pip install -r data/ingest/requirements.txt
cd data/ingest
python3 simple_wikipedia.py
```

Re-runs are deterministic — output is sorted by `id`, JSON-serialised with `ensure_ascii=True`, so diffs only reflect real article-content changes upstream. Commit the regenerated `data/seed/wiki_passages.en.jsonl` alongside whatever change motivated the rerun.

## Sweep tests

Two sweep tests pin the retrieval defaults against the 90-passage corpus. Both live in [primer-kb-load/tests/](src/crates/primer-kb-load/tests/), not under `primer-knowledge/`, because they exercise the full ingestion + retrieval pipeline end-to-end.

- [retrieval_sweep.rs](src/crates/primer-kb-load/tests/retrieval_sweep.rs) — BM25-only sweep. Always built; `#[ignore]`'d so it doesn't run in default CI. Run via `cargo test -p primer-kb-load --test retrieval_sweep -- --ignored sweep_retrieval_params --nocapture`. The 24-cell sweep is what produced today's defaults: `KB_FINAL_TOP_K = 5` and `KB_BM25_ONLY_MIN_SCORE = 0.5`.
- [retrieval_sweep_hybrid.rs](src/crates/primer-kb-load/tests/retrieval_sweep_hybrid.rs) — hybrid (BM25 + dense-vector RRF) sweep. Gated on `--features fastembed` AND `#[ignore]`'d, so even `cargo test --features fastembed` skips it without `--ignored`. Run via `cargo test -p primer-kb-load --features fastembed --test retrieval_sweep_hybrid -- --ignored sweep_retrieval_params_hybrid --nocapture`. The 54-cell sweep produced today's hybrid defaults: `KB_BM25_TOP_K = 30`, `KB_VECTOR_TOP_K = 30`, `KB_FINAL_TOP_K = 5`, `RRF_K = 60`. Hybrid achieves 100% loose / 100% strict recall on the 87-query benchmark (with 20 strict-subset canonical-id mappings).

The standing regression tests (always-on, not `#[ignore]`'d) live in [retrieval_quality.rs](src/crates/primer-kb-load/tests/retrieval_quality.rs) (BM25-only) and [retrieval_quality_hybrid.rs](src/crates/primer-kb-load/tests/retrieval_quality_hybrid.rs) (hybrid; the real-`fastembed` recall floor is gated on the feature, but a `StubEmbedder` structural sanity check runs on every build).

The list of queries the BM25-only path is allowed to miss lives in [tests/common/mod.rs](src/crates/primer-kb-load/tests/common/mod.rs) as `KNOWN_FAILING_QUERIES`. The hybrid analogue, `KNOWN_FAILING_QUERIES_HYBRID`, is empty today — the dense leg lifted every BM25 strict miss in the benchmark, including the canonical "how does the sun shine" case (closed issue #42). New entries on the hybrid list mean either a real semantic regression or a corpus-coverage gap the dense leg can't bridge; investigate before committing.

---

### Recipe — Add or tune seed passages

You want to add a passage on a topic the corpus doesn't cover well, or tweak an existing passage so a struggling query finally resolves. The flow is JSONL-first: the data file is the source of truth, and the test suite tells you whether the change earned its keep.

**1. Edit the JSONL.** Choose the right layer. Hand-drafted, CC0 content goes in `data/seed/seed_passages.<pack>.jsonl`; new wiki content goes through the [Wikipedia ingestion pipeline](#wikipedia-ingestion-pipeline) and lands in `wiki_passages.<pack>.jsonl`. To add a third layer (say, a curriculum source), create a new `<your_layer>.<pack>.jsonl` in `data/seed/`; the discovery code picks it up automatically.

A row looks like this:

```jsonl
{"id":"sun_shine_01","source":"hand-drafted-cc0","license":"CC0-1.0","attribution":"The Primer seed corpus","source_url":null,"text":"The Sun shines because of nuclear fusion in its core. Hydrogen atoms collide so fast they fuse into helium, releasing energy as light and heat.","topics":["space","sun","fusion"]}
```

`id` is the deduplication key — make it stable and unique. `source` doubles as the `sources`-table foreign key; reuse an existing source id (see existing rows) unless you are introducing a new one. `text` is the passage body (the field is `text`, not `body`). `topics` is informational only and not persisted.

**2. Register the source if it is new.** If you used an `id`/`source` that doesn't appear elsewhere in the JSONL, the loader will create a `sources` row from the per-row `license`/`attribution`/`source_url` fields automatically. Just make sure those fields are correct on every row that uses the new source — the loader takes whichever value it sees first.

**3. Run the BM25 quality test.** This is the always-on regression check; it must stay green:

```bash
cd src
cargo test -p primer-kb-load --test retrieval_quality
```

If a previously-failing query now passes, remove its entry from `KNOWN_FAILING_QUERIES` in [tests/common/mod.rs](src/crates/primer-kb-load/tests/common/mod.rs). The defensive check there will reject stale entries on the next run.

**4. (Optional but encouraged) Run the BM25 sweep.** This re-confirms the tuned defaults still hold up after your corpus change:

```bash
cargo test -p primer-kb-load --test retrieval_sweep -- --ignored sweep_retrieval_params --nocapture
```

**5. Run the hybrid quality + sweep.** If you have `fastembed` set up locally:

```bash
cargo test -p primer-kb-load --features fastembed --test retrieval_quality_hybrid
cargo test -p primer-kb-load --features fastembed --test retrieval_sweep_hybrid -- --ignored sweep_retrieval_params_hybrid --nocapture
```

If a previously-failing hybrid query now passes, remove its entry from `KNOWN_FAILING_QUERIES_HYBRID`.

**6. Commit JSONL + test-list change in one commit.** A single commit per "I improved retrieval on X" keeps the history reviewable. Include the recall delta in the commit message body if you have it from the sweep output.

### Recipe — Add a new locale

Adding a new locale means picking a stable pack id, writing a translation pack, providing a seed corpus, and exercising the locale-guard on resume. There are no schema migrations involved — locale-scoped tables are created on demand the first time `open_for_locale` sees the new pack.

**1. Add the variant to `Locale`.** Edit [primer-core/src/i18n.rs](src/crates/primer-core/src/i18n.rs). At time of writing, `English` and `German` already exist; this recipe uses Japanese (`"ja"`) as a fresh worked example. Pick a stable ISO 639-1 pack id (`"ja"`, `"hi"`, `"fr"`, …); it is what every per-locale file and table is keyed by, so it should not change once you start using it.

**2. Add the TOML pack.** Mirror `primer-core/i18n/en.toml` at `primer-core/i18n/<pack_id>.toml`. Every locale-specific user-visible string lives there, including `break_suggestion_intro` with the `{minutes}` substitution token. The pattern is reusable for any future locale-aware unit substitution — the locale's TOML owns its own unit word ("minutes" / "Minuten" / etc).

**3. Add the seed JSONL.** At minimum, create `data/seed/seed_passages.<pack_id>.jsonl` with hand-drafted passages. Optionally regenerate `data/seed/wiki_passages.<pack_id>.jsonl` via the [ingestion pipeline](#wikipedia-ingestion-pipeline) — Simple English Wikipedia obviously won't apply, but the same pattern adapts to any per-language MediaWiki extracts endpoint.

**4. Open the KB for the new locale.** No code change beyond the call site:

```rust
let kb = SqliteKnowledgeBase::open_for_locale(path, Locale::Japanese)?;
```

The first open creates `passages_ja`, `passages_ja_content`, and `embeddings_ja` automatically. Existing locale tables in the same file are untouched. Subsequent opens re-check via `CREATE IF NOT EXISTS`, so the operation is idempotent.

**5. Test the resume guard.** Open a session under the new locale, save it, then try to resume with `--language` set to a different pack. The CLI must error with the two-resolution hint (drop `--language`, or pass `--language <stored>`); the longitudinal vocabulary record depends on this. Without the guard, `concepts.concept_language_tag` would silently start tagging new rows under the wrong locale.

**6. Run the locale-related tests.**

```bash
cd src
cargo test -p primer-pedagogy locale
cargo test -p primer-knowledge locale
```

**7. Cross-link to chapter 5 on storage.** Per-learner locale is stored alongside the session because mismatching `concept_language_tag` would silently corrupt the longitudinal vocabulary record — the same reason the resume guard exists. See chapter 5 for the `learners.locale` field added in storage schema v6.
