# Primer Roadmap

This roadmap is organised around one principle: **get a working conversation loop as fast as possible, then improve every layer independently**. The trait architecture means we can swap inference backends, add speech, integrate hardware, and refine pedagogy without rewriting the core.

---

## Phase 0 — Cloud-Backed Proof of Concept

**Goal:** A text-mode Primer that holds a genuine Socratic conversation with a child, running on any machine with Rust and an internet connection.

**Runs on:** MacBook, Spark DGX, any Linux box, RedMagic 11 Pro (via Termux or adb shell).

**No special hardware required.** This is where all three contributors can work in parallel right now.

### 0.1 — Make the cloud backend conversationally useful

- ✅ Wire up SSE streaming in `CloudBackend` — done. NDJSON streaming for `OllamaBackend` landed in the same pass. Tokens drip through to the terminal as they arrive.
- ✅ Add a `--model` CLI flag to select between Claude models (sonnet for speed, opus for depth) — done. Defaults to `claude-sonnet-4-6`; required for ollama.
- ✅ Handle API errors gracefully — done. `PrimerError::Inference` now carries a typed `InferenceError` (`Auth | RateLimited{retry_after} | ServiceUnavailable | NetworkUnavailable | ModelNotFound{model} | Other`). Both cloud (Anthropic) and Ollama backends classify pre-stream failures and retry transient ones via `primer_core::retry::retry_with_backoff` (default 3 attempts, jittered exponential backoff, honors `Retry-After` ≤ 5 s). User-facing messages render via `primer_core::i18n::render_inference_error` — single i18n boundary; future locale support adds variants without touching producers. Mid-stream errors still propagate cleanly and the partial Primer turn drops. See [docs/superpowers/specs/2026-05-04-graceful-api-errors-design.md](docs/superpowers/specs/2026-05-04-graceful-api-errors-design.md).
- ✅ Add conversation persistence — sessions persist to SQLite via `primer-storage` (per-turn save). Default location is `~/.primer/<slug-of-name>.db`; explicit path via `--session-db <path>`. See [docs/superpowers/specs/2026-04-30-session-persistence-sqlite-design.md](docs/superpowers/specs/2026-04-30-session-persistence-sqlite-design.md)
- ✅ Resume past session — `--resume <uuid>` CLI flag loads a stored `Session` and picks up without a greeting. Errors clearly when the file or id is missing.
- ✅ Long-term memory across the context window — schema v2 added a rolling LLM-generated `Session.summary` (refreshed on resume + every 20 turns) plus FTS5 retrieval (`turn_text_fts`) of relevant older turns; both inject into the system prompt while the chat-message timeline stays = `recent_turns(window)`. Context budget is bounded across hours of conversation.

### 0.2 — Knowledge base bootstrapping

- ✅ Hybrid retrieval pipeline (BM25 + dense-vector RRF) — landed (PR #34, 2026-05-05; specs at [docs/superpowers/specs/2026-05-05-hybrid-retrieval-design.md](docs/superpowers/specs/2026-05-05-hybrid-retrieval-design.md) and [docs/superpowers/specs/2026-05-05-kb-bootstrapping-design.md](docs/superpowers/specs/2026-05-05-kb-bootstrapping-design.md)). New `primer-core::embedder::Embedder` trait, new `primer-core::rrf::fuse` Reciprocal Rank Fusion helper (`k = 60`), new `primer-embedding` crate (`StubEmbedder` always built; `FastEmbedBackend` with BGE-M3 default behind the `fastembed` feature; `OllamaEmbedder` behind the `ollama` feature). `KnowledgeBase::retrieve_hybrid` and `SessionStore::retrieve_session_turns_hybrid` both run BM25 + cosine in parallel and fuse via RRF; default trait impls fall back to BM25-only when no embedder is wired so every consumer can call them unconditionally. Knowledge-base schema bumped to v3 (`embedding_models` lookup table + per-locale `embeddings_<pack>` BLOB column); session-store schema bumped to v8 (mirror tables for per-turn vectors). Mismatched dim under an existing model name is a hard error in both stores. Configurable via `--embedder-backend none|stub|fastembed|ollama` (default `none`), `--embedder-model`, `--embedder-ollama-url`. Embedding-on-save runs in a `tokio::spawn` after `save_session`, fire-and-forget; idempotent at the storage layer.
- ✅ JSONL ingestion + auto-seed + `--reembed` infrastructure — landed in PR #34. New `primer-kb-load` crate exposes `load_jsonl(kb, path)` (idempotent on passage id), `auto_seed_if_empty(kb, locale)` (discovers `seed_passages.<pack>.jsonl` via `$PRIMER_SEED_DIR` → `$XDG_DATA_HOME/primer/seed/` → walking up from `CARGO_MANIFEST_DIR`), and `reembed_kb(kb, embedder, force, batch_size)` (backfills embeddings for passages missing one; `--force` re-embeds everything under a new model). Standalone binary `primer-kb-load --reembed --knowledge-db <path> --locale <pack>` is feature-gated on `fastembed`.
- ✅ Initial seed corpus — 56 hand-drafted CC0-1.0 passages across all five planned clusters at `data/seed/seed_passages.en.jsonl` (space: sun, moon, planets, stars, galaxies, black holes, comets, eclipses, tides, big bang, day/night; body: heart, lungs, brain, digestion, skeleton, muscles, skin, immune system, eyes, ears, sleep/dreams, where babies come from; how-things-work: magnets, electricity, batteries, simple machines, sound, light, mirrors/lenses, fire, ice/water, salt, soap; life: photosynthesis, seeds, flowers, food chains, cells, evolution, dinosaurs, mammals, birds, reptiles/amphibians, insects; earth/weather: rain, snow, clouds, wind, lightning, rainbows, water cycle, volcanoes, earthquakes, mountains, climate vs weather). Auto-loads on first run when the KB is empty.
- ✅ Retrieval-quality test in CI — `primer-kb-load/tests/retrieval_quality.rs` loads both seed files via `auto_seed_if_empty` into a temp DB and runs 58+ canonical child queries across every cluster (incl. wiki-targeted probes like "what is gravity", "what is a virus", "what is climate change"), asserting at least one top-5 passage mentions the expected key terms. Runs on every push so corpus edits that regress retrieval get caught early.
- ✅ Simple English Wikipedia ingestion (Phase 0.2 MVP, 2026-05-06; PR #36 merged at commit `44a70b8`; spec at [docs/superpowers/specs/2026-05-06-wikipedia-ingestion-design.md](docs/superpowers/specs/2026-05-06-wikipedia-ingestion-design.md), plan at [docs/superpowers/plans/2026-05-06-wikipedia-ingestion.md](docs/superpowers/plans/2026-05-06-wikipedia-ingestion.md)). Python pipeline at `data/ingest/` ingests 35 Simple English Wikipedia Science articles via the MediaWiki extracts API (lead-only chunking, batched 20-titles-per-request fetch, disambiguation-page detection, hand-curated whitelist seeded from Wikipedia Vital Articles). Output JSONL ships in-repo at `data/seed/wiki_passages.en.jsonl` (CC-BY-SA-3.0; per-passage `source_url` carries the canonical Wikipedia URL through to the `sources` table). `primer-kb-load::auto_seed_if_empty` extended to load all `*.<pack>.jsonl` files in the seed dir, so the wiki layer auto-loads alongside the existing CC0 seed corpus on a fresh KB. A future locale's wiki layer (e.g. `wiki_passages.de.jsonl`) auto-loads on a same-locale session without code change. Deferred follow-ups for the ingest tooling: #37 (real User-Agent contact identifier), ✅ #38 (429/5xx backoff in `fetch_leads` — landed 2026-05-09), #40 (aggregate per-source attribution for the wiki layer), #41 (tighten the disambiguation regex if false positives appear).
- ✅ Tune `RetrievalParams` defaults against the broader corpus — landed (Phase 0.2 close, 2026-05-06; plan at [docs/superpowers/plans/2026-05-06-retrieval-tuning.md](docs/superpowers/plans/2026-05-06-retrieval-tuning.md)). 87-query benchmark with 20 strict-subset canonical-id mappings drives an `#[ignore]`'d 24-cell `(top_k × min_score)` BM25 sweep at `src/crates/primer-kb-load/tests/retrieval_sweep.rs`; tuned production defaults are `KB_FINAL_TOP_K = 5` and `KB_BM25_ONLY_MIN_SCORE = 0.5`, with `RetrievalParams::default()` now reading from `primer_core::consts` as the single source of truth. The `retrieval_quality.rs` regression test pins production defaults with both loose-substring and strict-canonical-id assertions.
- ✅ Tune `HybridParams` defaults via the 54-cell hybrid sweep — landed (Phase 0.2.5 close, 2026-05-08). Sweep harness at `src/crates/primer-kb-load/tests/retrieval_sweep_hybrid.rs` (`--features fastembed --ignored`) measured all `(bm25_top_k × vector_top_k × final_top_k × rrf_k)` combinations against BGE-M3 on the 87-query benchmark. Selected production defaults `(bm25_top_k=30, vector_top_k=30, final_top_k=5, rrf_k=60)` achieve **100% loose / 100% strict recall** — lifting the BM25-only strict miss for "how does the sun shine" (#42, closed). `HybridParams::default()` now reads from `primer_core::consts` with a drift-prevention test, mirroring `RetrievalParams::default()`. New regression test `tests/retrieval_quality_hybrid.rs` ships in two flavours: `hybrid_retrieval_structural_with_stub` (always built; `StubEmbedder`; structural sanity only — no recall floor since stub vectors are noise on synonyms) and `hybrid_retrieval_recall_with_fastembed` (`--features fastembed`; real BGE-M3; pins 100/100 recall at `HybridParams::default()`). #39 closed.
- ✅ Tripwire diagnostic for `KB_BM25_ONLY_MIN_SCORE` floor — landed (2026-05-09). New `#[ignore]`'d test at `src/crates/primer-kb-load/tests/bm25_floor_tripwire.rs` probes the actual top-K BM25 score distribution across the benchmark and asserts `min > floor` and `median > floor × 2.0`. The 0.5 floor is a no-op for recall today (after the issue #45 corpus expansion: worst-case top-1 is 2.06, min top-K is 0.95, median 4.25 — all comfortably above), but the tripwire fires loudly if future corpus expansion dilutes scores enough that the floor would start filtering genuine hits. Pure helpers (`min_score`, `median_score`, `sorted_ascending`) carry their own unit tests. Closes issue #44.
- ✅ Reintroduce 4 paraphrase queries dropped during Phase 0.2 closeout — landed (2026-05-09; tracks issue #45). Benchmark expanded from 87 → 91 queries (24 strict-subset canonical-id mappings, was 20). All 4 paraphrases initially sat in `KNOWN_FAILING_QUERIES` (BM25-only path picks lexically adjacent passages — sleep-dreams/heart/photosynthesis — instead of the semantically correct one); the hybrid recall test then demonstrates the lift. **Hybrid lifted 2 of 4** out of the box ("what is inside a tiny bug" → seed:en:insects; "why does the brain need oxygen from the lungs" → seed:en:brain). The remaining 2 ("what makes my tummy growl when I am hungry", "why do flowers smell nice") were corpus-coverage gaps even BGE-M3 could not bridge — the canonical passage never named the symptom or scent in those words.
- ✅ Issue #45 full closure — corpus expansion to lift the remaining 2 paraphrases — landed (2026-05-10). Added a sentence describing stomach growling to `seed:en:digestion` ("Have you ever heard your tummy growl when you are hungry? When the stomach and small intestine are mostly empty, the muscles in their walls keep squeezing anyway…"). Added a new hand-drafted CC0 `seed:en:flowers` passage (~360 words) covering pollination, petals, scent, fragrant flowers, nectar, and pollinator attraction. Re-targeted the `"why do flowers smell nice"` benchmark canonical_id from `wiki-simple:en:plant` (which discusses photosynthesis/anatomy, not scent) to the new `seed:en:flowers` (which directly answers the child-style question). Both queries are now removed from `KNOWN_FAILING_QUERIES` AND from `KNOWN_FAILING_QUERIES_HYBRID` and serve as permanent regression guarantees. The hybrid recall test now passes 100% loose / 100% strict on all 91 queries / 24 mappings (no skipped queries). `KNOWN_FAILING_QUERIES` retains the other 2 paraphrases as a documented BM25-vs-hybrid-lift demonstration, and the BM25-only floor tripwire confirms the corpus expansion shifted the worst-case top-1 from 2.11 → 2.06 (still well above the 0.5 floor). Closes issue #45 in full.
- ✅ HTTP 429 / 5xx backoff for `data/ingest/simple_wikipedia.py::fetch_leads` — landed (2026-05-09; closes issue #38; spec at [docs/superpowers/specs/2026-05-09-fetch-leads-backoff-design.md](docs/superpowers/specs/2026-05-09-fetch-leads-backoff-design.md)). New pure-helper module `data/ingest/retry.py` exposes `is_retryable_status`, `parse_retry_after`, `compute_delay`, and a single composing `retry_http_get` that all three strategy fetchers (Simple English single-title, Simple English batch, Klexikon per-title) route through. Defaults: 3 attempts × 0.5 s × 2 + ±10% jitter ≈ ~1.5 s worst case before failure. `Retry-After` is honoured for delta-seconds form up to a 30 s budget; longer waits raise `RetryCapExceeded` immediately so the developer can re-run later. Network errors propagate unchanged (out of scope per #38). `RetrySettings.default()` reads from module consts with a drift-guard test. Python tests grew from 87 → 131 (43 new on `retry.py` + 1 integration test in `test_fetch_lead.py`); Rust workspace tests unchanged at 605/0/2.
- ✅ Klexikon corpus expansion 24 → 66 articles — landed (2026-05-10). Hand-curated 42 additional German children's-curriculum titles spanning the same five clusters as the English seed (added: Galaxie, Komet, Universum, Sternbild, Mondfinsternis to space; Skelett, Knochen, Muskel, Haut, Zahn, Ohr, Nase, Magen, Blut, Verdauung to body; Säugetier, Reptil, Fisch, Dinosaurier, Hund, Katze, Biene, Schmetterling, Baum, Blume, Wald, Pilz to life; Berg, Fluss, Meer, See, Wüste, Schnee, Wind, Jahreszeit to earth/weather; Atom, Schwerkraft, Energie, Elektrizität, Feuer, Luft, Temperatur to how-things-work). Pre-validated all candidates against Klexikon's `action=query` API to catch redirects (14 of 42 redirected to a canonical form — most often plural, sometimes a synonym/broader topic such as `Berg` → `Gebirge`, `Universum` → `Weltall`, `Atom` → `Atome und Moleküle`) and missing pages (none found) before staging the whitelist; the script transparently follows `redirects=1` so canonical titles land in the JSONL and slugs are computed from the resolved title. `Molekül` was deliberately omitted because it shares the canonical `Atome und Moleküle` redirect target with `Atom` and would have produced an id collision. The ingest path now hard-fails on post-resolution duplicate ids (a structural defence against this collision shape, replacing the manual probe). Output is `data/seed/wiki_passages.de.jsonl` (66 unique ids, all schema-valid, all texts ≥50 chars). Python tests grew from 132 → 135 (3 new on `_assert_unique_passage_ids` covering the passing case, the redirect-collision shape, and the error-message contract). Rust workspace unchanged at 605/0/2.
- ✅ German retrieval-quality benchmark — landed (2026-05-11; PR #63). Mirror of the English benchmark for the `de` locale. New `tests/common/de.rs::QUERIES_DE` carries 26 child-style German queries with 20 strict-subset `wiki-klexikon:de:*` canonical-id mappings, distributed across the five non-wiki clusters (space, body, how-things-work, life, earth/weather). Parallel test files `tests/retrieval_quality_de.rs` (BM25-only, always built) and `tests/retrieval_quality_hybrid_de.rs` (structural `StubEmbedder` always built + real-`fastembed` recall floor) pass for every query at the production defaults; both `KNOWN_FAILING_QUERIES_DE` and `KNOWN_FAILING_QUERIES_DE_HYBRID` ship empty as regression tripwires. Sanity floors enforce ≥20 queries, ≥15 strict mappings, ≥3 queries per cluster, and that every declared `canonical_id` exists in the shipped corpus. The shared `Cluster` enum is reused; the `Wiki` variant is unused for `de` because the Klexikon layer IS the German wiki — there is no parallel hand-drafted seed sitting alongside it (if one is added later, update the cluster-floor sanity test which enumerates explicit variants). Rust workspace tests 605 → 648 (+43 from the new test binaries' shared sanity-test duplication). Stress-paraphrase queries (4-6 deliberately misaligned child phrasings to demonstrate the hybrid-over-BM25 lift the way EN issue #45 did) are queued as a follow-up issue — every query in this initial set was authored against the targeted passage's lead text, so the BM25 leg is being measured against vocabulary it was effectively given.
- ✅ German retrieval-sweep diagnostics — landed (2026-05-11). Parallel to the EN sweep tooling: `tests/retrieval_sweep_de.rs` (24-cell BM25, `#[ignore]`'d, default features) and `tests/retrieval_sweep_hybrid_de.rs` (54-cell hybrid, `#[ignore]`'d, `--features fastembed`). Same algorithmic-winner rule as the EN sweeps; the DE BM25 sweep is sized to five clusters (Klexikon IS the wiki for `de`, so `Cluster::Wiki` is dropped). On the 66-passage Klexikon corpus + 26-query benchmark: BM25 strict recall flatlines at 100% from `top_k=5` onward regardless of `min_score` (so production `(top_k=5, min_score=0.5)` is a comfortable choice); hybrid hits 100% loose / 100% strict in every one of the 54 cells, confirming that BM25 alone already clears the German benchmark for this corpus/query mix. No per-locale default tuning required at this corpus size — the diagnostics serve as drift guards for future corpus expansion. Workspace test count 648 → 658.
- ✅ German stress-paraphrase queries — landed (2026-05-14; closes issue #64). Benchmark expanded from 26 → 31 queries (25 strict-subset canonical-id mappings, was 20). 5 child-style German paraphrases added with deliberately mis-aligned vocabulary: `"warum macht mein bauch komische geräusche wenn ich hunger habe"` → `verdauung`, `"wieso wird es im sommer länger hell als im winter"` → `jahreszeiten`, `"warum bekomme ich gänsehaut wenn mir kalt ist"` → `haut`, `"warum gibt es ebbe und flut am meer"` → `mond`, `"warum sind blätter im herbst bunt"` → `baum`. BM25 at production defaults misses 3 (verdauung/haut/mond — surfaces lexically adjacent passages like energie/schall, klima/temperatur, fluss/meer/wetter); the other 2 (jahreszeiten, baum) pass BM25 because of partial vocabulary overlap with the article lead. **Hybrid lifts 1 of 3** out of the box — `verdauung` rises into top-5 via the dense leg. The remaining 2 (`gänsehaut wenn mir kalt ist`, `ebbe und flut am meer`) are genuine corpus-coverage gaps: Klexikon's `haut` article doesn't describe the goosebumps reflex, and Klexikon's `mond` article doesn't discuss tides — no embedding can bridge content that isn't there. Resolution mirrors EN issue #45's option (b): both stay in `KNOWN_FAILING_QUERIES_DE_HYBRID` with documented rationale rather than ship a hand-drafted German seed layer (Klexikon IS the children's wiki for German). `KNOWN_FAILING_QUERIES_DE` carries the 3 BM25-only misses as a documented BM25-vs-hybrid-lift demonstration. Workspace test count unchanged at 739 (sanity tests don't grow when only data expands).
- ✅ German (`de`) locale rollout: Klexikon (children's wiki) ingest path — landed (2026-05-09). The Phase 0.2 ingest pipeline at `data/ingest/simple_wikipedia.py` was generalised around a frozen `WikiSource` dataclass (`pack_id`, `api_url`, `web_base_url`, `id_prefix`, `human_label`, `license`, `topic_tags`, `disambiguation_patterns`, `fetch_strategy`, `batch_size`) so multiple Wikipedia-shaped sources share one fetch / passage-emit / JSONL-write code path. Two presets ship today: `SIMPLE_ENGLISH` (Phase 0.2 path; `text_extracts` strategy; CC-BY-SA-3.0) and `KLEXIKON` (German children's wiki at `klexikon.zum.de`; `klexikon_wikitext` strategy; CC-BY-SA-4.0). Klexikon was selected over regular `de.wikipedia.org` because its hand-written ages-8-13 vocabulary fits the Primer's audience the way Simple English fits the English audience. Klexikon's MediaWiki has no TextExtracts extension, so the new strategy fetches `action=parse&prop=wikitext&section=0` and runs the result through `strip_klexikon_wikitext` — a pure-function wikitext-to-plaintext stripper that drops gallery blocks, image+category drop-block links, references, templates, and HTML comments while flattening regular wikilinks. `slugify` was upgraded from `str.lower()` to `str.casefold()` so ß→ss and umlauts decompose cleanly (was producing the corrupted slug `strae` for `Straße`). Initial 24 children's-curriculum articles seeded at `data/seed/wiki_passages.de.jsonl` (since expanded to 66 — see follow-up bullet); auto-loads end-to-end on `--language de` via the existing `auto_seed_if_empty` multi-file discovery. Python tests grew from 43 → 82 (39 new tests covering `WikiSource` invariants, wikitext stripping, Klexikon fetch path, and end-to-end pipeline). Rust workspace tests unchanged at 605/0/2.

- 🟡 Hindi (`hi`) locale rollout — preview scaffolding landed (2026-05-15; PR #105; spec at [docs/superpowers/specs/2026-05-15-hindi-locale-preview-rollout.md](docs/superpowers/specs/2026-05-15-hindi-locale-preview-rollout.md), plan at [docs/superpowers/plans/2026-05-15-hindi-locale-preview.md](docs/superpowers/plans/2026-05-15-hindi-locale-preview.md)). `Locale::Hindi` added to the enum and reachable via `--language hi`, but deliberately excluded from `Locale::ALL` until a native speaker reviews the machine-translated prompt pack. Two firewalls keep end users from accidentally reaching Hindi: (1) `Locale::ALL` exclusion (CLI/GUI pickers iterate that slice); (2) new `[meta] status = "preview"` field on prompt packs — closed `PackStatus { Stable, Preview }` enum exposed via `PromptPack::status()`, loader emits one `tracing::warn!` per `(process, locale)` pair via `load_cached`. Ships: `prompts/hi.toml` (~210 lines of Devanagari with `# REVIEW:` markers above translation blocks); `render_hindi` with 6 Devanagari error variants; `hi_IN-rohan-medium` Piper voice + multilingual `ggml-small.bin` Whisper pinned in `LOCALE_DEFAULTS` (~540 MB total); status page at `docs/localisation/hi/README.md`; model-eval skeleton at `docs/locale/models/HINDI.md`. **Not in scope** (deferred to future PRs): no Hindi corpus (no Klexikon-shaped Hindi children's wiki exists; candidates documented: NCERT, Pratham Books StoryWeaver, Wikisource Hindi), no `tests/common/hi.rs` queries, no retrieval-quality / sweep tests for `hi`, no GUI surface change. Architectural cleanup along the way: `locale_defaults` was promoted from inside the feature-gated `voice_loop/` module to the `primer-speech` crate root (zero speech-backend deps; 4 previously dormant regression tests now run in default CI). Flipping Hindi from preview to stable is a single follow-up commit: `[meta] status = "stable"` + `Self::Hindi` added to `Locale::ALL`. Rust workspace tests 777 → 800 (+5 from Hindi scaffolding + 18 from misc cleanup).

### 0.3 — Pedagogical engine refinement

- ✅ Implement concept extraction from child responses — landed via the `primer-extractor` crate (`ConceptExtractor` trait, `LlmConceptExtractor` + `StubConceptExtractor` impls). One LLM call per completed (child, primer) exchange returns separated `{ child_concepts, primer_concepts }`; spawned async after primer save, persisted via `SessionStore::update_turn_concepts`, applied to in-memory `LearnerModel.concepts` at the next-turn boundary
- ✅ Add LLM-based comprehension assessment: after the child responds, use a short classifier prompt to gauge whether the response demonstrates genuine understanding, parroting, or confusion — landed as `primer-comprehension` crate (Phase 0.3): `LlmComprehensionClassifier` produces per-concept `{depth, confidence, evidence}` assessments against the extractor's candidate set; results promote `LearnerModel.concepts.depth` via monotonic max (threshold-gated) AND persist to schema-v5 `turn_comprehensions` for offline depth-trajectory analysis.
- ✅ Implement learner model persistence (SQLite) so the Primer remembers what a child knows across sessions — landed via the `LearnerStore` trait + schema v4 migration; CLI startup adopts existing `sessions.learner_id` on first-run-after-upgrade
- ✅ Per-child vocabulary tracking with spaced repetition — landed (Phase 0.3, 2026-05-05; spec at [docs/superpowers/specs/2026-05-05-vocabulary-spaced-repetition-design.md](docs/superpowers/specs/2026-05-05-vocabulary-spaced-repetition-design.md)). New `primer-core::vocab` module with three pure functions (`apply_box_transition`, `is_due`, `due_concepts`) implements a Leitner-box scheduler with intervals `[1d, 3d, 7d, 14d, 30d]`. `apply_comprehension` advances the box on every accepted assessment; `build_turn_prompt` injects the top-K most-overdue concepts as a passive prompt-section that the LLM weaves in only if topically relevant. Schema v7 adds `learner_concepts.box_level INTEGER NOT NULL DEFAULT 0`. Configurable via `--vocab-max-per-prompt N` (default 4). All learner data stays per-DB. **Schema constraint preserved:** vocabulary entries continue to live as `ConceptState` rows so the Phase 4 anonymisation/contribution path is unchanged
- ✅ Add session time tracking with gentle break suggestions — landed (Phase 0.3 close, 2026-05-05; spec at [docs/superpowers/specs/2026-05-05-session-break-suggestion-design.md](docs/superpowers/specs/2026-05-05-session-break-suggestion-design.md)). New `PedagogicalIntent::SuggestBreak` (id=9) is seeded automatically into existing DBs on next open. `decide_intent_at_with_pack` consults `primer_core::session_timing::should_suggest_break_now` after engagement-state overrides (a frustrated child past the threshold gets `Scaffolding`, not `SuggestBreak`). Cadence resets on each fire. Locale-aware `{minutes}` interpolation via `pack.break_suggestion_intro(minutes)`. Configurable via `--session-break-after-mins N` (default 30; must be ≥1). The Primer never enforces a session halt — children can keep going through any number of nudges. `PedagogyConfig.break_suggest_after_minutes` is the renamed field (was `max_session_minutes`).
- ✅ Write unit tests for `decide_intent()` — characterization tests landed (18 tests in `prompt_builder::tests`); they pin down current behaviour and document the gaps below
- ✅ Make `Encouragement` reachable from `decide_intent` — landed in the engagement-classifier work (Phase 0.3): `LlmEngagementClassifier` supplies `EngagementAssessment` into `decide_intent`; "frustrated but still trying" now routes to `Encouragement`, "frustrated and stuck" to `Scaffolding`
- ✅ Detect factual-question patterns in `decide_intent` so "what is X?" / "how does Y work?" route to `DirectAnswer`, with `AnswerThenPivot` on the next turn — landed in the engagement-classifier work; both intents are now reachable
- ✅ Make `Disengaging` session-length-aware — route to `Encouragement` early in a session, `SessionClose` only after a meaningful duration — landed in the engagement-classifier work using session turn count as the duration proxy

### 0.4 — Developer experience

- ✅ OpenAI-compatible inference + embedding backend — landed (2026-05-15; PRs #105 + #106; spec at [docs/superpowers/specs/2026-05-15-openai-compat-backend-design.md](docs/superpowers/specs/2026-05-15-openai-compat-backend-design.md), plan at [docs/superpowers/plans/2026-05-15-openai-compat-backend.md](docs/superpowers/plans/2026-05-15-openai-compat-backend.md)). `OpenAiCompatBackend` in `primer-inference` speaks `/v1/chat/completions` with SSE streaming, error classification (401/403 → Auth, 429 → RateLimited, 404+model → ModelNotFound, 5xx → ServiceUnavailable), optional Bearer auth, and bounded jittered retry on the pre-stream phase. `OpenAiCompatEmbedder` in `primer-embedding` speaks `/v1/embeddings` with native batch support (single request per batch, unlike `OllamaEmbedder`'s one-at-a-time loop). CLI flags `--backend openai-compat`, `--openai-compat-url`, `--openai-compat-api-key`, `--embedder-backend openai-compat`, `--embedder-openai-compat-url`, `--embedder-openai-compat-model` all wired through `primer-engine`. Unblocks talking to oMLX, LM Studio, vLLM, llama.cpp `--server`, plus remote providers (Together, Groq, OpenRouter) that share the OpenAI API shape — particularly ~20–40% throughput gains on Apple Silicon via MLX-native servers. Follow-ups: GUI wiring (deferred until CLI path is smoke-tested against a real server).
- [ ] Add `cargo test` coverage for all crates (core trait contracts, prompt builder output, dialogue manager state transitions, knowledge base retrieval) — partial. 223 tests across the workspace after the learner-model-persistence landing (Phase 0.3): streaming parsers + stub-summarize (19 in `primer-inference`), `decide_intent`/prompt builder + `dialogue_manager` including resume + summary refresh + classifier integration + learner-store wiring (67 in `primer-pedagogy`), session persistence + v2/v3/v4 migration + load + FTS retrieval + `turn_classifications` + `LearnerStore` round-trip (83 in `primer-storage`), classifier trait + `LlmEngagementClassifier` + `StubEngagementClassifier` (12 in `primer-classifier`), CLI slug + path resolution + adoption-flow round-trip (21 in `primer-cli`), enum-variant arrays + speech traits (9 in `primer-core`, 10 in `primer-speech`). `primer-knowledge` retrieval is the only crate still without coverage.
- [ ] Set up CI (GitHub Actions) — build + test on Linux and macOS
- ✅ Add a `CLAUDE.md` to the repo with codebase conventions — done.
- ✅ Add `--verbose` flag that prints pedagogical decisions (intent chosen, knowledge passages retrieved, engagement state) alongside the conversation — landed in the engagement-classifier work; `[intent]` and `[classifier]` lines emit to stderr when `--verbose` is passed
- ✅ `.env` / `~/.primer_env` auto-loading via dotenvy — done. Secrets can live in either a project-local `.env` or a user-global `~/.primer_env`.

**Phase 0 exit criteria:** You can sit a child in front of a terminal, type `cargo run --bin primer -- --backend cloud --name Binti --age 8`, and have a 15-minute Socratic conversation about a topic of their choosing that feels qualitatively different from ChatGPT. The Primer asks more questions than it answers. It catches parroting. It suggests breaks. It remembers what was discussed last time. Phase 0.1, Phase 0.2, and Phase 0.3 are all complete; the only remaining Phase 0 work is the developer-experience items in 0.4 (CI setup, broader retrieval test coverage). Next milestone is Phase 1 (local llama.cpp inference).

---

## Phase 1 — Local Inference

**Goal:** Run the conversation loop without internet, using a local LLM on whatever hardware is available.

### 1.1 — llama.cpp integration

- [ ] Implement `LlamaCppBackend` via llama-cpp-rs bindings (or direct FFI)
- [ ] Support GGUF model loading from a configurable path
- [ ] Benchmark Qwen3 7B Q4_K_M on: MacBook M-series (Metal), DGX (CUDA), RedMagic 11 Pro (Vulkan)
- [ ] Add automatic backend fallback: try local model first, fall back to cloud if local is too slow or unavailable
- [ ] Add a 3B model fallback path for constrained devices

### 1.2 — Qualcomm NPU (RedMagic / Snapdragon 8 Elite)

- [ ] Implement `QnnBackend` using QNN SDK for Hexagon NPU inference
- [ ] Target: 15+ tok/s on Qwen3 7B W4A16 quantisation
- [ ] Test thermal behaviour under sustained conversation (the phone in an enclosure will need monitoring)

### 1.3 — Hybrid inference

- [ ] Implement the inference router: use local model for routine Socratic turns, escalate to cloud for complex reasoning or knowledge-intensive questions
- [ ] Add latency-aware routing — if local generation is taking >5s, offer to switch to cloud
- [ ] Config-driven: parents can set "local only" (no network), "cloud preferred" (best quality), or "hybrid" (default)

**Phase 1 exit criteria:** The same conversation from Phase 0 works offline, with acceptable latency (<3s to first token on at least one local platform).

---

## Phase 2 — Speech

**Goal:** Talk to the Primer instead of typing.

### 2.1 — Speech-to-text

- [x] Implement `WhisperBackend` for STT — `StreamingSpeechToText` trait + `WhisperStt` backend. Streaming session emits partial transcript chunks via `StreamingSpeechToText::begin_session`. Landed in voice round-trip POC.
- [x] Add voice activity detection (VAD) so the Primer knows when the child has finished speaking — Silero VAD via vendored `silero-vad-rust 6.2.1` (with 2-line `is_multiple_of` patch). Landed in voice round-trip POC.
- [ ] Handle ambient noise gracefully (these are children's rooms, not recording studios)

### 2.2 — Text-to-speech

- [x] Implement `PiperBackend` for TTS
- [ ] Select/create a warm, patient voice profile (the Primer should sound like a favourite teacher, not a GPS)
- [x] Implement streaming TTS: begin speaking before generation is complete (sentence-boundary chunking)
- 2026-05-02 — Streaming `StreamingTextToSpeech` trait + Piper backend (vendored + ort-rc.10 patch). Smoke binary `tts_hello`. PR #6.

### 2.3 — Conversation flow with speech

- [ ] Echo cancellation (the Primer's own voice shouldn't trigger STT)
- [x] Interrupt handling (if the child starts talking while the Primer is speaking, stop and listen) — cancel-on-SpeechStart in `speech_loop`: a `SpeechStart` event mid-LATENT_THINK aborts the in-flight LLM future and waits for the next `SpeechEnd` before retrying. Landed in voice round-trip POC.
- [ ] Silence handling (long pauses during thinking are normal for children — don't rush them)
- [x] Voice round-trip POC (`--speech` mode) — end-to-end LISTEN → LATENT_THINK → SPEAK → LISTEN state machine in `primer-cli::speech_loop`. `primer-speech::cpal_io` (`MicCapture`, `SpeakerSink`, `Resampler`). `rust-toolchain.toml` pins Rust 1.87+. `--speech`, `--whisper-model`, `--voice-onnx`, `--voice-config`, `--voice`, `--mic-silence-ms` CLI flags. 10+ mock-driven `speech_loop` tests + 5 `cpal_io` unit tests. 2026-05-02.

**Phase 2 exit criteria:** A child can have the Phase 0 conversation entirely by voice, with the Primer speaking responses aloud.

---

## Phase 3 — Hardware Enclosure

**Goal:** A physical device a child can hold.

### 3.1 — Display

- [ ] Integrate a colour E Ink display (7-8", capacitive touch) or repurpose a tablet display
- [ ] Design a child-friendly UI: large text, illustrations for concepts, no distracting chrome
- [ ] Support both text and illustrated conversation modes

### 3.2 — Audio hardware

- [ ] MEMS microphone array integration (beam-forming for noise rejection)
- [ ] Speaker selection and integration
- [ ] Physical volume control (no software-only volume — children need a knob)

### 3.3 — Power and enclosure

- [ ] Battery management (target: 5-7 hours active use)
- [ ] Thermal design (passive cooling for the compute module)
- [ ] Drop-resistant enclosure (this will be held by children)
- [ ] Physical power button with a satisfying click

### 3.4 — Compute module integration

- [ ] Mount the chosen SBC or phone-as-compute-module in the enclosure
- [ ] Wire display, audio, and power
- [ ] Boot-to-Primer: the device turns on and is immediately ready for conversation

**Phase 3 exit criteria:** A child can pick up the device, turn it on, and have the Phase 2 conversation without any other equipment.

---

## Phase 4 — Pedagogical Depth

**Goal:** The Primer becomes a genuinely effective learning companion, not just a Socratic chatbot.

- [ ] Curriculum alignment — map the concept graph to real educational standards (Australian Curriculum, IB PYP) so parents and teachers can see what's being explored
- [ ] Multi-session learning arcs — the Primer notices that a child has been circling a concept across several sessions and designs a sequence to deepen understanding
- [ ] Parental dashboard — a simple web UI showing what the child has been exploring, where they're strong, where they're struggling (read-only, no surveillance)
- [ ] Collaborative mode — two children sharing a Primer, with the dialogue engine adapting to multiple learners
- [ ] Assessment without testing — the Primer continuously assesses understanding through conversation, never through quizzes or scores
- [ ] Aggregated age-appropriate language corpus — opt-in, explicitly parent-consented contribution of (age, technical-term, plain-language explanation, child-comprehension signal) tuples to a shared dataset for fine-tuning future small models on genuinely age-calibrated language. PII scrubbing happens on-device before anything leaves: child names, family members, locations, and any free-form notes are stripped. The vocabulary store from Phase 0.3 is the source. No data ever leaves a device without an explicit per-child, per-export consent action by a parent

---

## Who works on what

The crate boundaries are designed so contributors can work in parallel without stepping on each other.

| Contributor | Natural focus areas |
|---|---|
| **Horst Herb** | `primer-pedagogy` (dialogue engine, prompt engineering), `primer-cli` (developer UX), `primer-knowledge` (corpus curation), integration testing, project coordination |
| **Bernd Brinkmann** *(EE/ML)* | `primer-inference` (local backends — llama.cpp, QNN, RKNN), Phase 3 hardware (display, audio, enclosure, power), thermal design |
| **Frithjof Herb** *(ML / math)* | `primer-inference` (model quantisation, benchmark harness), `primer-pedagogy` (comprehension assessment classifier, learner model ML), `primer-knowledge` (embedding-based semantic search) |
| **Claude** *(Anthropic AI pair-programmer, via Claude Code)* | Implementation pairing on Phase 0 work, code review, refactoring, test scaffolding, documentation upkeep — under Horst's direction. Commits that include AI work are tagged with a `Co-Authored-By: Claude …` trailer. |

Bernd and Frithjof are listed as planned contributors — both are currently busy with other commitments and have not yet committed code. The Phase 1 hardware/local-inference work is genuinely waiting on them. Everyone can contribute to Phase 0 immediately: it's pure Rust, runs anywhere, and needs no special hardware.

---

## Principles that don't change

These hold across all phases:

1. **The Primer asks more questions than it answers.** If a phase makes the Primer more answer-y, something went wrong.
2. **The Primer does not maximise engagement.** It suggests breaks. It lets children stop. It never guilt-trips.
3. **All learner data stays on-device.** Cloud inference is stateless. The learner model never leaves the device without explicit parental consent.
4. **The pedagogical engine is testable without hardware.** Every phase must keep the stub backend working. If you can't test the Socratic behaviour in a terminal, the architecture is wrong.
