# Getting started

Welcome! This developer manual is for open-source contributors who have just cloned The Primer and want to get a working build, run their first conversation, and understand where things live before making changes. If you have used Rust before, the steps below should take ten to fifteen minutes; if anything trips you up, the **Gotcha** callouts in this chapter are the issues people hit most often.

This chapter walks you through prerequisites, cloning, the first stub-mode run, then each optional backend (cloud, Ollama, embeddings, voice). It ends with a tour of testing, logging, and where the conversation data lives. Later chapters go deeper into the architecture and the contribution workflow.

## Prerequisites

You need:

- **rustup** (NOT Homebrew rust). Install from <https://rustup.rs>. The repo's `rust-toolchain.toml` pins the toolchain version that the build expects.
- **(optional) espeak-ng** — only needed if you plan to run `--speech` mode. macOS: `brew install espeak-ng`. Debian/Ubuntu: `sudo apt install espeak-ng-data`.
- **(optional) An Anthropic API key** — only needed if you plan to run the cloud backend. Free Ollama and the no-network stub backend both work without one.

> **Gotcha:** Homebrew rust shadows rustup on `$PATH` and ignores `rust-toolchain.toml`. The `silero` and `whisper` features need Rust 1.87+, which Homebrew may not yet ship. Always invoke as `~/.cargo/bin/cargo` for primer commands, or remove Homebrew rust from your `$PATH`. If a build fails with a confusing edition or feature-flag error, this is almost always why.

## Cloning and the workspace layout

> **Gotcha:** The workspace root is `src/Cargo.toml`, not the repo root. Every cargo command runs from `src/`. There is no `Cargo.toml` at the repo root, so `cargo build` from the top-level directory will fail with "could not find `Cargo.toml`".

```bash
git clone https://github.com/<org>/primer.git
cd primer/src
cargo build
```

The first build pulls dependencies and compiles twelve workspace crates, which takes a few minutes on a fresh checkout. SQLite is bundled and TLS uses rustls, so there are no system C libraries to install for the default build.

## First run (stub backend, no API key)

The stub backend returns canned Socratic responses with no model and no network. It is the fastest way to confirm your build works:

```bash
cargo run --bin primer
```

You will see a greeting and a prompt. Type a question, hit return, and a stub response comes back. Type `quit`, `exit`, or `bye` to leave.

> **Note:** The binary is named `primer`, not `primer-cli`, because that is the `[[bin]]` name in the CLI crate's `Cargo.toml`. Use `cargo run --bin primer` even though the crate directory is `primer-cli/`.

## Cloud backend (Anthropic)

The cloud backend streams responses from the Anthropic Messages API. It needs an `ANTHROPIC_API_KEY` in your environment.

The CLI auto-loads `.env` files at startup from two locations: a project-local `.env` (searched up from the current working directory) and a user-global `~/.primer_env`. Project-local wins over the home file, so a per-repo `.env` overrides anything you have set globally. Both `.env` and `~/.primer_env` are gitignored.

```bash
cp ../.env.example ../.env
# edit ../.env to add ANTHROPIC_API_KEY=sk-ant-...
cargo run --bin primer -- --backend cloud --name Binti --age 8
```

`--name` and `--age` are used to build the learner profile and to tune the system prompt for the child's age. The default cloud model is `claude-sonnet-4-6`; override with `--model claude-opus-4-7` (or any current Anthropic model id).

## Ollama backend

If you have [Ollama](https://ollama.com) running locally, you can point the Primer at it instead of the cloud:

```bash
cargo run --bin primer -- --backend ollama --model llama3.2
```

`--model` is required for Ollama and must match a tag you have already pulled (`ollama pull llama3.2`). The default endpoint is `http://localhost:11434`; override with `--ollama-url`.

## Embeddings (hybrid retrieval)

By default, the Primer's knowledge base uses BM25-only retrieval. To enable the hybrid (BM25 + dense-vector) retrieval pipeline, build with the `embedding` feature and pass `--embedder-backend fastembed`:

```bash
cargo run --bin primer --features primer-cli/embedding -- --embedder-backend fastembed
```

> **Note:** First run downloads BGE-M3 (~570 MB) into `~/.cache/primer/models/`. This is expected, not a hang — subsequent runs use the cached copy. The download uses the same `cdn.pyke.io` ONNX-runtime path as the speech feature.

If the download fails, the Primer falls back to BM25-only and logs a warning rather than refusing to start.

## Voice mode (speech)

Voice mode swaps the text REPL for a full LISTEN -> THINK -> SPEAK -> LISTEN loop using Silero VAD, Whisper STT, and Piper TTS. It is gated by the `speech` Cargo feature so default builds stay light.

> **Gotcha:** `--speech` requires system espeak-ng for Piper to phonemise text. The bundled `espeak-rs` ships an incomplete subset that fails on most voices. macOS: `brew install espeak-ng`. Debian/Ubuntu: `sudo apt install espeak-ng-data`.

You will also need a whisper.cpp model file and a Piper voice (matching `.onnx` and `.onnx.json` sidecar). Piper voices live at <https://huggingface.co/rhasspy/piper-voices>; whisper.cpp models at <https://huggingface.co/ggerganov/whisper.cpp>.

```bash
cargo run --bin primer --features primer-cli/speech -- \
  --speech \
  --whisper-model <path>.bin \
  --voice-onnx <path>.onnx \
  --voice-config <path>.onnx.json \
  --voice <model-id>
```

`--voice` is the `VoiceProfile.model_id` and must match the file stem of `--voice-onnx` — Piper rejects mismatches at session open.

## Tests, logging, verbose output

The workspace ships with parser, dialogue-manager, and integration tests across every crate.

```bash
cargo test                              # all tests
cargo test -p primer-pedagogy           # one crate
cargo test -p primer-pedagogy decide    # by substring
RUST_LOG=debug cargo run --bin primer
cargo run --bin primer -- --verbose     # pedagogy-flow tracing on stderr
```

`RUST_LOG=debug` enables verbose tracing across the whole codebase. `--verbose` is a lighter-weight option that prints just the pedagogical decisions (`[intent]`, `[classifier]`, `[extractor]`, `[comprehension]`) to stderr while keeping stdout clean for the conversation transcript.

## Where session data lives

By default, every conversation is persisted to `~/.primer/<slugified-name>.db` — one SQLite file per child, separate from the knowledge-base corpus on privacy grounds. The first time the directory is created, the CLI prints a one-line banner explaining where the data lives and how to opt out.

If you want a one-off in-memory session that disappears when you exit, pass `--no-persist`. This is mutually exclusive with `--resume` and `--session-db`.

## What to read next

- [02-architecture-overview.md](02-architecture-overview.md) for the big picture — the twelve-crate layered graph, the trait boundaries, and the data-flow through a single turn.
- [09-contributing.md](09-contributing.md) for the PR workflow, commit-message conventions, and how reviews work.
