# Developer / contributor manual

Welcome. This manual is for **external open-source contributors** who want to understand how the Primer is built, find their way around the workspace, and land changes that fit the project's pedagogical and architectural intent.

It covers: how to clone and run the project, how the fifteen crates fit together, where the pedagogical "soul" lives, how each subsystem (inference, retrieval, storage, classifiers, speech) is structured, how to test and debug locally, and how to send a pull request that we can merge with confidence.

It does **not** cover: the product vision (see [README.md](../../README.md) and [primer_technical_spec.md](../../primer_technical_spec.md)), the long-range phase plan ([ROADMAP.md](../../ROADMAP.md)), or speculative ideas not yet on the roadmap ([SPECULATIONS_AND_IDEAS.md](../../SPECULATIONS_AND_IDEAS.md)). It is also **not** a user manual — it assumes you intend to read or change code.

> **Note:** if you are an AI agent reading this, the same content (in tighter form, without the welcome) lives in [CLAUDE.md](../../CLAUDE.md) at the repo root. The two files are kept consistent by hand; if they ever drift, CLAUDE.md is authoritative for agent-facing conventions and this manual is authoritative for narrative explanation.

## Reading paths

Three suggested orderings depending on why you are here:

- **I'm new and want to contribute** → start with [Getting started](01-getting-started.md), then [Architecture overview](02-architecture-overview.md), then [Contributing](09-contributing.md), then pick a subsystem chapter that interests you.
- **I want to add feature X** → look up the recipe in the [How do I…?](#how-do-i) table below; each recipe links into the relevant chapter and gives numbered steps.
- **I want to understand subsystem Y** → jump straight to its chapter. Chapters 03–07 each stand on their own; chapter 02 is recommended pre-reading but not required.

## Chapters

| # | Chapter | What it covers |
|---|---|---|
| 01 | [Getting started](01-getting-started.md) | Clone, build, first run, env files, tests, logging. |
| 02 | [Architecture overview](02-architecture-overview.md) | Trait-based abstraction, the 15-crate graph, pedagogical principles. |
| 03 | [Inference and pedagogy](03-inference-and-pedagogy.md) | `InferenceBackend` impls (stub/cloud/ollama/openai-compat/llamacpp/qnn + fallback/router), `DialogueManager`, `decide_intent`, retry, error i18n. |
| 04 | [Knowledge and retrieval](04-knowledge-and-retrieval.md) | FTS5 + hybrid retrieval, embedders, seed corpus, locales. |
| 05 | [Storage and sessions](05-storage-and-sessions.md) | Session/learner stores, schema migrations, long-term memory. |
| 06 | [Classifiers and the learner model](06-classifiers-and-learner-model.md) | Classifier/extractor/comprehension trio, vocab Leitner box. |
| 07 | [Speech and the voice loop](07-speech-and-voice-loop.md) | VAD/STT/TTS, vendored crates, speech_loop state machine. |
| 08 | [Testing and debugging](08-testing-and-debugging.md) | Test layout, RUST_LOG, common pitfalls, debugging recipes. |
| 09 | [Contributing](09-contributing.md) | PR workflow, commits, style, where to find open work. |

## How do I…?

| Task | Recipe |
|---|---|
| Add a new `InferenceBackend` | [03-inference-and-pedagogy.md#recipe--add-a-new-inferencebackend](03-inference-and-pedagogy.md#recipe--add-a-new-inferencebackend) |
| Add a new `PedagogicalIntent` | [03-inference-and-pedagogy.md#recipe--add-a-new-pedagogicalintent](03-inference-and-pedagogy.md#recipe--add-a-new-pedagogicalintent) |
| Add or tune seed passages | [04-knowledge-and-retrieval.md#recipe--add-or-tune-seed-passages](04-knowledge-and-retrieval.md#recipe--add-or-tune-seed-passages) |
| Add a new locale | [04-knowledge-and-retrieval.md#recipe--add-a-new-locale](04-knowledge-and-retrieval.md#recipe--add-a-new-locale) |
| Add a schema migration | [05-storage-and-sessions.md#recipe--add-a-schema-migration](05-storage-and-sessions.md#recipe--add-a-schema-migration) |
| Add a structured-output classifier | [06-classifiers-and-learner-model.md#recipe--add-a-structured-output-classifier](06-classifiers-and-learner-model.md#recipe--add-a-structured-output-classifier) |
| Add a speech backend | [07-speech-and-voice-loop.md#recipe--add-a-speech-backend](07-speech-and-voice-loop.md#recipe--add-a-speech-backend) |
| Diagnose a quiet classifier | [08-testing-and-debugging.md#recipe--diagnose-a-quiet-classifier](08-testing-and-debugging.md#recipe--diagnose-a-quiet-classifier) |
| Run / test / debug locally | [08-testing-and-debugging.md](08-testing-and-debugging.md) |

## Conventions used in this manual

The chapters use a small set of callout markers so the eye can skip to the bit it needs:

- `> **Note:**` — an aside that doesn't change behaviour, just adds context.
- `> **Gotcha:**` — something that has bitten contributors (or the maintainer) before. Almost every CLAUDE.md "gotcha" appears as one of these in the relevant chapter; if you find a behaviour that surprised you and there is no gotcha for it yet, that is a great PR.
- `> **Why:**` — rationale for a non-obvious design choice. When you see one of these, the answer to "why didn't they just do X instead?" is in the box.
- `> **Recipe — <Action>:**` — a worked example with numbered steps. Recipes are linked from the [How do I…?](#how-do-i) table above.

Inline code references use markdown links pointing at the relevant file (and sometimes a line range), e.g. `primer-pedagogy/src/dialogue_manager/turn.rs`. Paths are relative to `src/` (the workspace root) unless the link explicitly starts with `../` to escape into the repo root for `README.md` / `ROADMAP.md` / similar.

Code blocks that show file content start with a path comment so you know where the snippet lives:

```rust
// src/crates/primer-core/src/lib.rs
pub trait InferenceBackend: Send + Sync { /* ... */ }
```

Shell commands assume you are in `src/` unless explicitly noted, because the workspace root is `src/Cargo.toml`, not the repo root.

## External resources

- [README.md](../../README.md) — product pitch, status, what works today.
- [ROADMAP.md](../../ROADMAP.md) — phase plan; what is built, what is next, what is deferred.
- [primer_technical_spec.md](../../primer_technical_spec.md) — long-form vision and design rationale.
- [CLAUDE.md](../../CLAUDE.md) — agent-facing conventions and gotchas (terser, denser, same content as this manual).
- [SPECULATIONS_AND_IDEAS.md](../../SPECULATIONS_AND_IDEAS.md) — open ideas that have not yet been planned.
- [LICENSE](../../LICENSE) — AGPL-3.0; see chapter 09 for what that means for contributors.
