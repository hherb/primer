# Developer / Contributor Manual — design spec

**Date:** 2026-05-08
**Status:** approved (pending spec review)
**Deliverable:** new `docs/devel/` tree, ~10 markdown files, targeting external open-source contributors.

---

## 1. Goals & non-goals

### Goals

- Give a stranger who just cloned the repo a clear path from "I have the source" to "I've built it, run it, and understand where to make my first change."
- Cover every subsystem in enough depth that a contributor can reason about a change without reading every crate's source first.
- Make "how do I do XYZ" lookups one click from the index — practical recipes with worked code samples, not just architecture description.
- Use a tone that welcomes newcomers (not the dense agent-facing prose of `CLAUDE.md`) while still respecting Rust fluency.

### Non-goals

- Not a replacement for `CLAUDE.md`. `CLAUDE.md` stays as the authoritative agent-facing source of truth for conventions and gotchas; this manual mirrors the same ground in human-readable form and links back where relevant.
- Not a Rust tutorial. Assume cargo workspace, traits, async/await fluency.
- Not pedagogical / product documentation. The "what is the Primer for" framing belongs to `README.md` and `primer_technical_spec.md`; the manual links out for that context but does not duplicate it.
- Not auto-generated API docs. `cargo doc` already covers per-item API. The manual covers shape, intent, and how-to.

---

## 2. Audience & relationship to existing docs

**Primary audience:** external OSS contributors. Assume Rust + cargo fluency; do not assume any Primer-specific knowledge.

**Relationship to other docs:**

| Doc | Role | Manual's relationship |
|---|---|---|
| `README.md` | Product pitch, user-facing status | Manual links to it from `index.md` and `01-getting-started.md`; does not duplicate. |
| `ROADMAP.md` | Phase plan | Manual links to it from `02-architecture-overview.md` (status) and `09-contributing.md` (where to look for open work). |
| `primer_technical_spec.md` | Long-form vision | Manual links to it from `02-architecture-overview.md` for context. |
| `CLAUDE.md` | Agent-facing conventions + gotchas | Manual covers the same conventions/gotchas in contributor-friendly tone with examples. CLAUDE.md stays authoritative; manual deep-links back where useful. |
| `docs/background_research/` | Architecture deep-dives (i18n, inference) | Manual links to specific files when a topic is covered in more depth there. |
| `docs/superpowers/specs/` | Design specs (including this one) | Manual links to specific specs as supporting reading; not part of the contributor flow. |

When a fact lives in CLAUDE.md verbatim (e.g. the `Other`-string-never-reaches-users discipline), the manual restates it briefly in its own words and links to the CLAUDE.md section for the canonical phrasing.

---

## 3. File layout

```
docs/devel/
├── index.md
├── 01-getting-started.md
├── 02-architecture-overview.md
├── 03-inference-and-pedagogy.md
├── 04-knowledge-and-retrieval.md
├── 05-storage-and-sessions.md
├── 06-classifiers-and-learner-model.md
├── 07-speech-and-voice-loop.md
├── 08-testing-and-debugging.md
└── 09-contributing.md
```

Numerical prefixes give a deterministic reading order. The `index.md` references chapters by title (not number) so files can be renumbered later without breaking links.

---

## 4. Per-file content

### 4.1 `index.md` — landing page

- One-paragraph framing: who this manual is for, what it covers, what it doesn't.
- **Reading paths** section — three suggested orderings:
  - "I'm new and want to contribute" → 01 → 02 → 09 → pick a chapter.
  - "I want to add feature X" → look up the recipe in the table below.
  - "I want to understand subsystem Y" → jump straight to the relevant chapter.
- **How do I…? lookup table** — flat list of every recipe in the manual, deep-linking to the recipe heading in its chapter. Anchors use the GitHub slug of `Recipe — Foo` (em-dash + space collapses to a double-hyphen):
  - Add a new `InferenceBackend` → `03-inference-and-pedagogy.md#recipe--add-a-new-inferencebackend`
  - Add a new `PedagogicalIntent` → `03-inference-and-pedagogy.md#recipe--add-a-new-pedagogicalintent`
  - Add a new locale → `04-knowledge-and-retrieval.md#recipe--add-a-new-locale`
  - Tune retrieval / add seed passages → `04-knowledge-and-retrieval.md#recipe--add-or-tune-seed-passages`
  - Add a schema migration → `05-storage-and-sessions.md#recipe--add-a-schema-migration`
  - Add a structured-output classifier → `06-classifiers-and-learner-model.md#recipe--add-a-structured-output-classifier`
  - Add a speech backend → `07-speech-and-voice-loop.md#recipe--add-a-speech-backend`
  - Diagnose a quiet classifier → `08-testing-and-debugging.md#recipe--diagnose-a-quiet-classifier`
  - Run / test / debug locally → `08-testing-and-debugging.md`
- **Conventions used in this manual** — explains the four callout types, the file:line link convention, and the code-sample format.
- **External links** — README, ROADMAP, primer_technical_spec, CLAUDE.md, GitHub Issues.

### 4.2 `01-getting-started.md`

- Prerequisites: rustup (NOT Homebrew rust — explain the silent-fail with the toolchain pin); optional system deps for opt-in features (espeak-ng for speech; nothing else).
- Cloning and the **`src/` workspace gotcha** — every cargo command runs from `src/`, the repo root has no `Cargo.toml`. Spelled out as a callout.
- First build (default, no API key needed): `cargo build && cargo run --bin primer` with stub backend.
- Running with cloud (Anthropic): env-file locations, `.env` vs `~/.primer_env`, the API key.
- Running with Ollama.
- Running with embeddings: `--features primer-cli/embedding --embedder-backend fastembed`. Note ~570 MB BGE-M3 download on first run.
- Running with speech: `--features primer-cli/speech`, espeak-ng, voice ONNX paths.
- Running tests: `cargo test`, `cargo test -p <crate>`, single test by substring.
- Logging: `RUST_LOG=debug` and `--verbose`.
- Where session DBs live (`~/.primer/<slug>.db`), how to opt out (`--no-persist`).
- "Now what?" — pointer to chapter 02 for architecture, chapter 09 for contributing.

### 4.3 `02-architecture-overview.md`

- The central design principle: **trait-based hardware abstraction**. Backend selection is a runtime config choice, not a code change. This is what allows Phase 0 (cloud) to share code with Phase 1 (local NPU) and Phase 2 (voice).
- The 12-crate layered graph as a **mermaid diagram** (see §5 for styling) plus an ASCII fallback.
- One-paragraph summary table: each crate name links to the relevant subsystem chapter, with a one-line "owns the X" description.
- The four pedagogical principles encoded in the prompt (asks more than answers; never engagement-maximising; local-first data; comprehension via transfer/probing). These are constraints on every change.
- Status: what's done (link to README) vs. what's TODO (link to ROADMAP).

### 4.4 `03-inference-and-pedagogy.md`

Topics:
- The `InferenceBackend` trait shape (`generate`, `generate_stream`, `summarize`).
- The three concrete backends: `StubBackend`, `CloudBackend` (Anthropic SSE), `OllamaBackend` (NDJSON). Mention the `Role::System` mapping divergence.
- `DialogueManager` as orchestrator: directory module layout (`lifecycle.rs`, `turn.rs`, `background.rs`, `retrieval.rs`, `summary.rs`, `learner_update.rs`, `apply.rs`).
- `prompt_builder` and `decide_intent()`: the heuristic, why characterization tests matter.
- The retry layer (`primer_core::retry::retry_with_backoff`) — pre-stream only; mid-stream errors propagate cleanly.
- The typed `InferenceError` and the i18n boundary — `Other`'s inner string is dev-facing only.

Recipes:
- **Recipe — Add a new `InferenceBackend`.** Eight numbered steps: implement trait, hand-roll streaming if needed (callout pointing to the SSE/NDJSON helpers), add typed-error mapping, hook `retry_with_backoff` for the pre-stream phase, register in `primer-cli`'s backend factory, add `--backend` enum variant, write integration tests using the stub pattern, update README.
- **Recipe — Add a new `PedagogicalIntent`.** Six steps: add the variant to the enum (the source of truth), add a routing branch in `decide_intent`, extend the prompt builder section selector, add a characterization test pinning the routing case, restart any DB to let the lookup-table seeder pick up the new variant, link to chapter 05 for the lookup-table mechanics.

### 4.5 `04-knowledge-and-retrieval.md`

Topics:
- `KnowledgeBase` trait, `Passage` type.
- `SqliteKnowledgeBase` schema: per-locale `passages_<pack_id>` FTS5 + `passages_<pack_id>_meta` + `embeddings_<pack_id>`. Cross-locale tables: `sources`, `embedding_models`. Schema at user_version 3.
- BM25 ranking: how the negative-rank flip works in the public API.
- Hybrid retrieval: `retrieve_hybrid`, BM25 leg + dense-vector cosine leg, RRF fusion via `primer_core::rrf::fuse`. `HybridParams::default()` reads from `primer_core::consts::retrieval`.
- The `Embedder` trait and three impls: `StubEmbedder` (always built; FNV+xorshift hash), `FastEmbedBackend` (BGE-M3, ~570 MB), `OllamaEmbedder`. The `embedding_models` invariant — mismatched dim is a hard error.
- `auto_seed_if_empty` and multi-file seed discovery: env precedence, lexicographic load order, multi-layer corpus (CC0 hand-drafted + CC-BY-SA-3.0 Wikipedia).
- Wikipedia ingestion pipeline at `data/ingest/` — Python; pure functions; network as the test boundary; how to regenerate.
- Sweep tests as the basis for default tuning (BM25-only and hybrid).

Recipes:
- **Recipe — Add or tune seed passages.** Four steps: edit the JSONL (id, body, source attribution), bump the sweep test's expected recall if needed, run the sweep (`cargo test -p primer-knowledge --test retrieval_sweep`), update `KNOWN_FAILING_QUERIES_HYBRID` in `tests/common/mod.rs` if a new query becomes resolvable.
- **Recipe — Add a new locale.** Seven steps: add `Locale` variant to `primer-core::i18n`, add the TOML pack file, add seed JSONL files (`seed_passages.<pack>.jsonl`, optionally `wiki_passages.<pack>.jsonl`), open the KB with `open_for_locale` (no migration step needed — tables are created on demand), test the locale guard on `--resume`, regenerate the wiki layer via `data/ingest/` if applicable, link to chapter 05 for the per-learner locale persistence.

### 4.6 `05-storage-and-sessions.md`

Topics:
- The privacy split: per-child session DB at `~/.primer/<slug>.db`, separate from the RAG corpus.
- `SessionStore` and `LearnerStore` traits.
- Schema-migration pattern: idempotent `CREATE IF NOT EXISTS` and `pragma_table_info` ALTER guards, transaction-wrapped, version bump, drift validation against Rust enums on every `open()`.
- The append-only invariant on `save_session` and why `update_turn_concepts` is the explicit backfill path.
- FTS5 query sanitisation (`sanitize_fts_phrase`).
- Long-term memory layered into the system prompt — summary + retrieved-older turns; the chat timeline stays linear.
- Turn-embedding-on-save fire-and-forget pattern.
- The current schema version (8) and what each version added (one-line summary per migration).

Recipes:
- **Recipe — Add a schema migration.** Six steps: bump `USER_VERSION` in `schema.rs`, write `apply_vN_migrations` following the v7/v8 template (callout with annotated example), wire it into the open path, add a migration round-trip test (open old DB → apply → assert schema), add a drift-validation entry if the migration touches a lookup-table-seeded enum, document the migration in CLAUDE.md's "Schema is at version N" line.

### 4.7 `06-classifiers-and-learner-model.md`

Topics:
- The classifier / extractor / comprehension trio: trait, `Llm*` impl wrapping `Arc<dyn InferenceBackend>`, `Stub*` for tests with `with_response` / `with_script`.
- The spawn / await / detach-on-timeout pattern, applied identically across all three.
- Soft-fail discipline: classifier/extractor/comprehension errors NEVER propagate; log via `tracing::warn!` and return an empty / unknown result.
- The classifier-extractor-comprehension chain inside the dialogue manager's background task; combined timeout budget; why both lag one turn.
- `LearnerModel` umbrella: `LearnerProfile` + `Vec<ConceptState>` + `LearningPreferences` + latest engagement snapshot. Edit the leaf, not the umbrella.
- `apply_comprehension` is monotonic-max; sub-threshold assessments still persist for offline analysis.
- Vocab Leitner-box scheduler in `primer_core::vocab`; passive review hints injected via the prompt builder, never drilled.
- Shared utilities: `extract_first_json_object` and `truncate_to_chars` in `primer_core::llm_util`.

Recipes:
- **Recipe — Add a structured-output classifier.** Nine steps: define the trait in your crate, add the `Llm*` impl (callout with the JSON-extraction pattern using `llm_util`), add the `Stub*` impl with `with_response`/`with_script`, add a `*Settings` struct with all numerics in `primer-core::consts`, add a schema migration if you need to persist (link to chapter 05), wire the spawn/await pattern into `DialogueManager`'s background module, add CLI flags following the `--classifier-*` template, add the soft-fail tracing-warn path, add stub-driven tests.

### 4.8 `07-speech-and-voice-loop.md`

Topics:
- The five speech traits all inheriting from `Named`: `VoiceActivityDetector`, `SpeechToText` + `StreamingSpeechToText`, `TextToSpeech` + `StreamingTextToSpeech`.
- Concrete backends: stubs + real backends behind features (`silero`, `whisper`, `piper`, `cpal`).
- Why `piper-rs 0.1.9` is vendored at `src/vendor/piper-rs/` — the `ort 2.0.0-rc.10` patch.
- Why `silero-vad-rust 6.2.1` is vendored at `src/vendor/silero-vad-rust/` — three patches; how to verify with `~/.cargo/bin/cargo`.
- The `speech_loop` state machine: LISTEN → THINK → SPEAK → LISTEN, no barge-in invariant in either direction.
- The `is_speaking` `AtomicBool` and the drain-hook discipline; why `spawn_blocking` matters for the synchronous wait.
- Speaker ringbuf sizing (240 000 samples ≈ 5 s at 48 kHz) — never shrink.
- Markdown-stripping for TTS; the `*` → " times " heuristic for digits.
- The dedicated audio-capture thread (not a tokio task); resampler tail-handling with `leftover` + flush sentinel + drain pulse.
- espeak-ng requirement and the data-directory probe.

Recipes:
- **Recipe — Add a speech backend.** Seven steps: pick the trait (VAD, STT, or TTS), implement it with `Named`'s `name()` impl once, gate behind a cargo feature in `primer-speech/Cargo.toml`, vendor or patch any incompatible deps under `src/vendor/` and add the `[patch.crates-io]` entry in `src/Cargo.toml`, wire into the `LoopBackends` factory in `primer-cli`, add a smoke example in `examples/`, add system-dep installation notes to `08-testing-and-debugging.md`.

### 4.9 `08-testing-and-debugging.md`

Topics:
- Test layout: per-crate `tests/`, shared mocks in `test_support.rs`, characterization tests for `decide_intent`, sweep tests for retrieval.
- Running tests: `cargo test`, `cargo test -p <crate>`, single test by substring; `--features` matrix for feature-gated crates.
- `RUST_LOG` and `--verbose`: when each is the right tool. The `[intent]` / `[classifier]` / `[extractor]` / `[comprehension]` line format.
- Inspecting session DBs: `sqlite3 ~/.primer/<slug>.db`, useful queries (most recent session, turn classifications, comprehensions).
- Common pitfalls (the "gotcha" parade — most of these are direct mirrors of CLAUDE.md gotchas):
  - Homebrew rust shadowing rustup → use `~/.cargo/bin/cargo`.
  - `cargo build` fails because you ran it from the repo root, not `src/`.
  - `--speech` panics with espeak data error → install `espeak-ng`.
  - `--embedder-backend fastembed` errors → enable the `embedding` feature.
  - First run of fastembed downloads ~570 MB → expected, not a hang.
  - First run of `--speech` downloads ONNX runtime → expected, not a hang.
  - Voice rejected with model-id mismatch → pass `--voice <model-id>` matching the ONNX filename.
  - Streaming hangs forever → check that you're not using a 2xx response with no body; mid-stream errors should propagate.
- How to debug a streaming hang (point at where to add tracing).
- How to debug a classifier/extractor that's not firing (`--verbose` + check the timeout knobs).

Recipe:
- **Recipe — Diagnose a quiet classifier.** Five steps: set `--verbose`, look for the `[classifier]` line, if missing inspect `turn_classifications` for the last session, increase `--classifier-timeout-ms`, swap to `--classifier-backend stub` to confirm the integration point isn't the LLM call.

### 4.10 `09-contributing.md`

- Repo workflow: fork, branch naming (`feature/...`, `fix/...`), PR conventions.
- Commit conventions: conventional commits style (visible in `git log`), no `--no-verify`, no `--amend` after push.
- Code style: `cargo fmt` + `cargo clippy --workspace --all-targets`. CI runs both; PRs that fail either get a cleanup pass.
- The "no magic numbers" rule: in this codebase, numerics live in `primer-core::consts` (if invariant) or in per-subsystem `*Settings` structs (if tunable) — never inline.
- The "categorical text columns are normalised" rule: link to chapter 05.
- Where to find open work: ROADMAP, GitHub Issues, the `// TODO` markers in source, `SPECULATIONS_AND_IDEAS.md`.
- How to ask for review: PR template, what reviewers will check (clippy clean, tests added, CLAUDE.md updated if conventions changed).
- License and contributor agreement (link out, not duplicated).

---

## 5. Conventions used in the manual

### 5.1 Callout types

Four blockquote-based callout types. Plain markdown means they render correctly on GitHub, in IDEs, and in offline viewers — no custom directives.

```
> **Note:** an aside that doesn't change behaviour.

> **Gotcha:** something that has bitten contributors before. Almost every CLAUDE.md "gotcha" becomes one of these.

> **Why:** rationale for a non-obvious design choice.

> **Recipe — Add a new X:** worked example with numbered steps and code blocks.
```

### 5.2 Code samples

- Fenced code blocks tagged with the language (`rust`, `bash`, `sql`, `toml`).
- Where the snippet lives in the repo, the first line of the block is a comment with the path: `// src/crates/primer-core/src/inference.rs`.
- For commands: prefix with `$` only when paired with output; otherwise bare command lines.
- Snippets stay short — the manual prefers a 5-line illustrative excerpt with a link to the file over a 40-line copy.

### 5.3 Code references in prose

Use the markdown link convention from the VSCode extension context:

- File: `[inference.rs](src/crates/primer-core/src/inference.rs)`
- File at line: `[dialogue_manager/turn.rs:120](src/crates/primer-pedagogy/src/dialogue_manager/turn.rs#L120)`

Never use bare backticks for file paths — readers in the IDE lose the click-through.

### 5.4 Mermaid diagram styling

Every diagram must be **legible on both light and dark GitHub themes** without modification. To achieve this:

- Use `%%{init: ...}%%` directives at the top of each diagram to set theme variables.
- Use **white text on saturated dark fills**, which gives ≥ 7:1 text-on-fill contrast (well above the WCAG 4.5:1 text threshold) and works identically against both light (`#ffffff`) and dark (`#0d1117`) page backgrounds.
- Pair every fill with a brighter stroke (the colours below are ~2x lighter than their fill) so node edges stay visible against the dark theme background; this exceeds the WCAG 3:1 threshold for graphical elements.
- Use `font-size: 16px` minimum for node labels, `14px` for edge labels.
- Keep node count per diagram under ~12; break larger graphs into multiple diagrams.
- Use `classDef` to apply consistent palette across diagrams in the manual.

**Manual-wide palette** (intended to read clearly on both themes):

| Role | Fill | Stroke | Text |
|---|---|---|---|
| Trait / interface (primer-core) | `#1e3a5f` (deep blue) | `#4a90d9` | `#ffffff` |
| Concrete impl (other crates) | `#2d5016` (deep green) | `#5cb85c` | `#ffffff` |
| External boundary (HTTP, FS, vendor) | `#5a3a1e` (deep amber) | `#d9a14a` | `#ffffff` |
| Stub / test only | `#4a4a4a` (neutral) | `#888888` | `#ffffff` |

Each chapter that uses these colours includes a one-line legend under the first diagram.

**Reference template** (used at the top of every diagram in the manual):

```mermaid
%%{init: {
  'theme': 'base',
  'themeVariables': {
    'fontSize': '16px',
    'fontFamily': 'system-ui, -apple-system, sans-serif',
    'primaryColor': '#1e3a5f',
    'primaryTextColor': '#ffffff',
    'primaryBorderColor': '#4a90d9',
    'lineColor': '#888888',
    'secondaryColor': '#2d5016',
    'tertiaryColor': '#5a3a1e'
  }
}}%%
graph LR
  classDef trait fill:#1e3a5f,stroke:#4a90d9,stroke-width:2px,color:#ffffff
  classDef impl  fill:#2d5016,stroke:#5cb85c,stroke-width:2px,color:#ffffff
  classDef ext   fill:#5a3a1e,stroke:#d9a14a,stroke-width:2px,color:#ffffff
  classDef stub  fill:#4a4a4a,stroke:#888888,stroke-width:2px,color:#ffffff
```

The crate-graph diagram in `02-architecture-overview.md` also ships an **ASCII fallback** (the existing one in `CLAUDE.md`) immediately after the mermaid block, so the manual remains useful in environments where mermaid does not render.

### 5.5 Recipe heading anchors

Each recipe uses an h3 heading of the form `### Recipe — Add a new InferenceBackend`. GitHub auto-generates the anchor (`#recipe--add-a-new-inferencebackend`); the index uses these anchors verbatim.

---

## 6. Out of scope (explicit non-deliverables)

- API reference docs. Use `cargo doc`.
- Per-method walkthroughs of `DialogueManager::respond_to_streaming`. Manual covers the orchestration shape; the source is the source.
- Build instructions for embedded targets (Phase 1 NPU work). Add when those backends land.
- Translations of the manual. English-only initially; add when locale stories close.

---

## 7. Out-of-band coordination

- After writing, the manual is committed in a single PR titled `docs(devel): contributor manual`.
- `README.md` gets a one-line addition: "**Contributing?** See [docs/devel/](docs/devel/) for the developer manual."
- `CLAUDE.md` gets no changes; the manual cross-links into it, not the other way around.

---

## 8. Success criteria

The manual succeeds when:

1. A new contributor can clone, build, and run the Primer with stub backend in under 10 minutes using only `01-getting-started.md`.
2. Every recipe has been validated by walking through it against the actual codebase — code samples paste-runnable, line numbers correct.
3. Every "gotcha" in `CLAUDE.md` either appears in the manual (as a `> **Gotcha:**` callout in the relevant chapter) or has been judged irrelevant to external contributors with a note in this spec.
4. The mermaid diagrams render legibly on GitHub light theme, GitHub dark theme, and at least one offline viewer (VS Code's preview).
5. Every internal link in the manual resolves; the index's "How do I…?" table reaches each recipe in one click.
