# Testing and debugging

This chapter is for the contributor who has cloned the repo, built per [chapter 1](01-getting-started.md), and now needs to run the test suite, get useful logs out of a running session, and diagnose the misbehaviour they were asked to fix. It does not introduce new architecture — it is a survival guide for working effectively in the codebase day to day.

The tools split roughly into three layers. First, `cargo test` and the per-crate test layout — most invariants in this codebase are pinned by tests, and knowing where they live tells you both what is safe to change and where to add coverage. Second, `RUST_LOG` and `--verbose` — the former exposes [tracing](https://docs.rs/tracing) output from every crate, the latter prints a small set of high-signal pedagogy-flow lines on stderr. Third, the SQLite session DB — when something the dialogue manager did or did not do looks wrong, the persisted record is the source of truth.

The chapter ends with a "common pitfalls" parade — most of these mirror gotchas in [CLAUDE.md](../../CLAUDE.md) — and two debugging recipes for the symptoms that come up most often.

## Test layout

Tests live next to the crate they test, never in a top-level `tests/` directory at the repo root. There are three patterns in use, each chosen for a reason:

- **Inline `#[cfg(test)] mod tests`** for unit tests against private items. The 30+ characterization tests for `decide_intent` live this way at the bottom of [primer-pedagogy/src/prompt_builder.rs](../../src/crates/primer-pedagogy/src/prompt_builder.rs) (search for `mod tests`). They pin the heuristic's *current* behaviour — what intent does the Primer pick for a frustrated 6-year-old who just asked "what is gravity?" — and they are the first thing to read when changing intent routing.
- **Per-crate `tests/` directory** for integration tests that exercise a whole crate's public surface. Examples: [primer-storage's session tests](../../src/crates/primer-storage/src/store/tests/session_tests.rs), [the retrieval-quality benchmarks](../../src/crates/primer-kb-load/tests/retrieval_quality.rs), and the BM25-only and hybrid-retrieval sweep harnesses at [retrieval_sweep.rs](../../src/crates/primer-kb-load/tests/retrieval_sweep.rs) and [retrieval_sweep_hybrid.rs](../../src/crates/primer-kb-load/tests/retrieval_sweep_hybrid.rs). The host-tested benchmark harness for inference throughput lives at [primer-inference/src/bench/](../../src/crates/primer-inference/src/bench/) — its pure metrics/thermal/prompt-loading modules run on the default `cargo test` (no NPU or GGUF needed), so the QNN and llama.cpp device examples carry no untested logic.
- **Per-axis split for the dialogue manager.** [primer-pedagogy/src/dialogue_manager/tests/](../../src/crates/primer-pedagogy/src/dialogue_manager/tests/) holds three files — `lifecycle_tests.rs`, `turn_tests.rs`, `background_tests.rs` — each pinned to one axis of behaviour. Shared mocks (stub backends, stub session stores, stub classifiers) and fixture builders live in [test_support.rs](../../src/crates/primer-pedagogy/src/dialogue_manager/test_support.rs) so the three test files don't fork their setup. When you add a new method to `DialogueManager`, add the test to the file matching its responsibility — don't grow a fourth test module.

> **Why:** the split mirrors the production split (`lifecycle.rs` / `turn.rs` / `background.rs`), so a reviewer reading a PR can hold one axis in their head at a time. A test that crosses axes is a smell — it usually means a method moved files but its test didn't.

The retrieval sweeps are worth calling out separately. [retrieval_sweep.rs](../../src/crates/primer-kb-load/tests/retrieval_sweep.rs) is a 24-cell grid search over BM25-only retrieval parameters; [retrieval_sweep_hybrid.rs](../../src/crates/primer-kb-load/tests/retrieval_sweep_hybrid.rs) is a 54-cell sweep over the hybrid (BM25 + dense vector) parameter space, gated behind `--features fastembed --ignored`. There are parallel German sweeps at [retrieval_sweep_de.rs](../../src/crates/primer-kb-load/tests/retrieval_sweep_de.rs) and [retrieval_sweep_hybrid_de.rs](../../src/crates/primer-kb-load/tests/retrieval_sweep_hybrid_de.rs) over the Klexikon corpus. All four files are thin `#[ignore]` shims over the shared harness at [tests/common/sweep/](../../src/crates/primer-kb-load/tests/common/sweep/), which issue #98 split into [mod.rs](../../src/crates/primer-kb-load/tests/common/sweep/mod.rs) (scaffold) + [bm25.rs](../../src/crates/primer-kb-load/tests/common/sweep/bm25.rs) (BM25 grid + selection) + [hybrid.rs](../../src/crates/primer-kb-load/tests/common/sweep/hybrid.rs) (`fastembed`-gated hybrid grid) — those submodules are the single source of truth for the grid constants, the selection rule, and the print format; adding a new locale is data-only (define `QUERIES_<XX>` in `tests/common/<xx>.rs` plus a ~50-line shim). Both produce CSVs for offline analysis. They are how the current defaults in [primer-core::consts::retrieval](../../src/crates/primer-core/src/consts.rs) were chosen, and they exist so the next time someone wants to retune, they can re-run the sweep and update the consts in one commit.

The [BM25 floor tripwire](../../src/crates/primer-kb-load/tests/bm25_floor_tripwire.rs) is a separate `#[ignore]`'d diagnostic — it probes the actual top-K BM25 score distribution and fires loudly if the margin above `KB_BM25_ONLY_MIN_SCORE` closes. Run it explicitly when expanding the seed corpus: `~/.cargo/bin/cargo test -p primer-kb-load --test bm25_floor_tripwire -- --ignored --nocapture`.

## Running tests

All test commands run from `src/`. Invoke cargo as `~/.cargo/bin/cargo` (the rustup proxy) so the pinned 1.88 toolchain in `rust-toolchain.toml` is honoured — a Homebrew `cargo` on `$PATH` silently uses its own (older) toolchain and produces confusing trait-resolution failures, especially on the speech features.

```bash
cargo test                                 # everything, default features
cargo test -p primer-pedagogy              # one crate
cargo test -p primer-pedagogy decide_intent  # filter by substring
cargo test -- --nocapture                  # show stdout/stderr from passing tests
```

> **Note:** the default `cargo test` now builds the `embedding` (fastembed) feature — it is in `default` for both `primer-cli` and `primer-gui`. The first run downloads the ONNX Runtime binary from `cdn.pyke.io`; CI proves this on Linux and macOS.

Feature-gated test paths need their feature on the command line. The most common ones:

```bash
cargo test -p primer-embedding --features fastembed
cargo test -p primer-speech    --features silero,whisper,piper,cpal
cargo test -p primer-speech    --features macos-native        # SFSpeechRecognizer / AVSpeechSynthesizer (macOS)
cargo test -p primer-speech    --features macos-native-26      # SpeechAnalyzer Swift sidecar (macOS 26)
cargo test -p primer-inference --features llamacpp             # in-process llama.cpp (MockLlamaEngine covers the seam on default build too)
cargo test -p primer-inference --features qnn                  # Qualcomm NPU host-mock path (no device needed)
cargo test -p primer-kb-load   --features fastembed -- --ignored  # hybrid sweep
```

> **Note:** `--ignored` runs tests marked `#[ignore]` *instead of* the default suite, not in addition to it. The hybrid sweeps (EN and DE) are `#[ignore]` because they download the BGE-M3 model on first run (~570 MB) and take minutes to complete — they are run on demand, not in CI.

> **Note:** most of the QNN and llama.cpp logic is host-tested on the *default* `cargo test` via trait-abstracted seams (`MockLlamaEngine`, the Genie host-mock) and the pure `primer-inference::bench` module. The `--features qnn` / `--features llamacpp` runs add the real-FFI arms; actual on-device throughput numbers stay device-gated (no GGUF or NPU is touched by any autonomous run). The macOS-native smoke tests (e.g. `tests/macos_tts.rs`, `tests/macos26_smoke.rs`) are `#[ignore]`'d and need a real mic / a macOS 26 host.

Single-test substring filtering works on every test target. `cargo test -p primer-pedagogy decide_intent_routes` matches every test whose name contains that substring, which is the fastest way to iterate on the decide-intent characterization suite while editing it.

## Cross-compiling and on-device validation

The whole `primer` binary dep graph cross-compiles cleanly to `aarch64-linux-android` from a stock macOS host (NDK 29 + the pinned 1.88 toolchain), and CI enforces this as a drift guard on every push/PR (`.github/workflows/ci.yml::android-cross-compile`). Reproduce locally:

```bash
rustup target add aarch64-linux-android --toolchain 1.88
cargo build --target aarch64-linux-android --bin primer   # NOT --workspace: primer-gui is webkit2gtk, out of Android scope
```

Android stays BM25-only by guidance (issue #157), so the CI guard pins the `primer-cli` steps to `--no-default-features` — the device-unverified aarch64 ort download is kept out of the required guard, and runtime guidance on Android remains `--embedder-backend none`.

Three runbooks cover the device side: [android-build-quickstart.md](android-build-quickstart.md) (the Tauri-Android APK build path), [redmagic-termux-quickstart.md](redmagic-termux-quickstart.md) (the Termux on-device REPL build, including the `/tmp`-not-writable and `--name`-consistency-on-resume gotchas), and [qnn-validation-runbook.md](qnn-validation-runbook.md) (the Qualcomm NPU bring-up procedure).

## RUST_LOG and `--verbose`

These are two different tools. Use the right one.

**`--verbose`** is a flag on `primer-cli`. When set, the REPL prints four high-signal pedagogy-flow lines on **stderr** at the end of each turn:

```
[intent] Curious -> SocraticQuestion
[classifier] Engaged conf=0.84 (llm:claude-sonnet-4-6)
             — child asked a follow-up about gravity, building on the previous turn
[extractor] child=["gravity", "weight"] primer=["mass", "force"] (llm:cloud:claude-sonnet-4-6)
[comprehension] gravity=Aware(0.55) mass=Familiar(0.78) (llm:cloud:claude-sonnet-4-6)
```

The lines are produced by the CLI directly from `dm.last_intent()`, `dm.last_assessment()`, `dm.last_extraction()`, and `dm.last_comprehension()` in [primer-cli/src/main.rs](../../src/crates/primer-cli/src/main.rs). They reflect the engine's view of the just-completed turn. Stdout stays clean — the child still sees only the Primer's response — so you can pipe stdout into a log file and read `--verbose` output live on the terminal.

> **Note on identifier asymmetry:** the classifier emits `llm:{model}`, while the extractor and comprehension emit `llm:{backend}:{model}`. The classifier shipped first and predates the convention; if you grep stderr for an identifier, account for both shapes.

**`RUST_LOG`** is the standard [`tracing-subscriber`](https://docs.rs/tracing-subscriber) env var. The CLI's default filter is `info,ort=warn,whisper_cpp_plus=warn,cpal=warn` — quiet enough for everyday use, but `info`-level events from the engine will print. Override with the usual idioms:

```bash
RUST_LOG=debug cargo run --bin primer                    # firehose
RUST_LOG=primer_pedagogy=debug cargo run --bin primer    # one crate
RUST_LOG=primer::retry=info cargo run --bin primer       # one tracing target
RUST_LOG=info,ort=info cargo run --bin primer -- --speech ...  # ONNX session-init logs
```

The retry helper at [primer-core::retry](../../src/crates/primer-core/src/retry.rs) emits one `info`-level event per retry decision under target `primer::retry`, which is how you confirm a flaky cloud call is being retried (and how a future voice-mode change can subscribe to play a "give me a moment" bridging phrase).

> **Note:** `RUST_LOG` and `--verbose` are independent. Use `--verbose` for the four-line pedagogy summary; use `RUST_LOG` for tracing crate output. Setting both is fine and often useful — the streams interleave on stderr.

## Inspecting session DBs

Every conversation is persisted to a per-child SQLite file at `~/.primer/<slugified-name>.db` unless `--no-persist` was passed. The schema is documented in [chapter 5](05-storage-and-sessions.md); the relevant tables for debugging are `sessions`, `turns`, `turn_classifications`, `turn_comprehensions`, `turn_concepts`, `concepts`, `learners`, and `learner_concepts`.

You don't need anything more than the system `sqlite3` CLI:

```bash
sqlite3 ~/.primer/binti.db
```

Useful starting queries:

```sql
-- most-recent session for this child
SELECT id, started_at, ended_at, summary_through_turn_index
FROM sessions
ORDER BY started_at DESC
LIMIT 1;
```

```sql
-- engagement classifications for the most-recent session, with state names
SELECT t.turn_index,
       es.name AS engagement,
       tc.confidence,
       c.identifier AS classifier
FROM turn_classifications tc
JOIN turns t              ON t.id = tc.turn_id
JOIN engagement_states es ON es.id = tc.engagement_state_id
JOIN classifiers c        ON c.id = tc.classifier_id
WHERE t.session_id = (SELECT id FROM sessions ORDER BY started_at DESC LIMIT 1)
ORDER BY t.turn_index;
```

```sql
-- per-concept comprehension assessments for the most-recent session
SELECT t.turn_index,
       co.name      AS concept,
       ud.name      AS depth,
       tcm.confidence,
       tcm.evidence
FROM turn_comprehensions tcm
JOIN turns t                ON t.id = tcm.turn_id
JOIN concepts co            ON co.id = tcm.concept_id
JOIN understanding_depths ud ON ud.id = tcm.depth_id
WHERE tcm.session_id = (SELECT id FROM sessions ORDER BY started_at DESC LIMIT 1)
ORDER BY t.turn_index, co.name;
```

```sql
-- learner's per-concept mastery state, sorted by most-recent encounter
SELECT co.name      AS concept,
       ud.name      AS depth,
       lc.confidence,
       lc.encounter_count,
       lc.box_level,
       lc.last_encountered
FROM learner_concepts lc
JOIN concepts co             ON co.id = lc.concept_id
JOIN understanding_depths ud ON ud.id = lc.depth_id
ORDER BY lc.last_encountered DESC
LIMIT 20;
```

> **Gotcha:** Categorical text columns are stored as integer foreign keys, not as TEXT. Joining against the lookup table (`engagement_states`, `understanding_depths`, `concepts`, `classifiers`) is mandatory — a query that selects `engagement_state_id` directly will return integers and look like data corruption to a fresh pair of eyes. See [chapter 5](05-storage-and-sessions.md) for why.

## Common pitfalls

Most of these are direct mirrors of [CLAUDE.md](../../CLAUDE.md). They show up here because they bite contributors more often than anything else.

> **Gotcha:** `cargo build` from the repo root fails with "could not find `Cargo.toml`". The workspace root is `src/Cargo.toml`. Run every cargo command from `src/`.

> **Gotcha:** A confusing edition or feature-flag error on a fresh checkout almost always means Homebrew rust is shadowing rustup on `$PATH`. The toolchain pin in `rust-toolchain.toml` is honored only by rustup proxy binaries. Invoke as `~/.cargo/bin/cargo`, or remove Homebrew rust from `$PATH`.

> **Gotcha:** `--speech` panics at startup with an espeak data error. Install `espeak-ng` system-wide (`brew install espeak-ng` on macOS, `sudo apt install espeak-ng-data` on Debian/Ubuntu). The `espeak-rs` crate ships an incomplete subset of espeak's data files that fails for most voices.

> **Gotcha:** `--embedder-backend fastembed` exits with a build hint. The `embedding` cargo feature is now in `default` for `primer-cli` and `primer-gui`, so a normal build has it — but a `--no-default-features` build does not, and on that build the flag exits with a "requires the `embedding` cargo feature" hint. The `--embedder-backend` *default* is feature-aware: `fastembed` when `embedding` is compiled in, `none` otherwise, so a flagless run does the right thing for whatever was built.

> **Gotcha:** First fastembed run hangs for a few minutes with no output. It is downloading the BGE-M3 model (~570 MB) into `~/.cache/primer/models/` from `cdn.pyke.io`. Subsequent runs are fast. If construction fails, the dialogue manager falls back to BM25-only with a `tracing::warn!` — the conversation still works.

> **Gotcha:** First `--speech` run hangs at compile time with no output. Cargo is downloading the ONNX Runtime binary (~50 MB) from `cdn.pyke.io`. Cached after the first build.

> **Gotcha:** Voice rejected at runtime with a model-id mismatch. `PiperTts::open_session(voice)` errors when the requested `model_id` doesn't match the one the backend was constructed with. Pass `--voice <model-id>` matching your `.onnx` filename (e.g. `--voice en_US-amy-medium` for `en_US-amy-medium.onnx`). Multi-voice runtime switching is deferred.

> **Gotcha:** Streaming hangs with no chunks ever delivered. The most common cause is a 2xx response with no body — the SSE / NDJSON parser is waiting for bytes that never come. Mid-stream errors should propagate cleanly; if they don't, see the recipe below for where to add tracing in the streaming buffer.

> **Gotcha:** `[classifier]` line never prints in `--verbose` output, and `turn_classifications` is empty. The classifier is timing out; small models (e.g. `gemma3:4b`) often need >500 ms for the structured-output call. Increase `--classifier-timeout-ms` (default 3000) or follow the recipe below.

> **Gotcha:** Session DB has the schema but no rows for a session you just ended. The dialogue manager saves on every turn, on `open_session`, on `resume_session`, and on `close_session`. Save failures `tracing::warn!` instead of propagating. The warnings fire from `primer_pedagogy::dialogue_manager` (not `primer_storage` — the storage layer just returns the error; the warn happens at the call site). Because `warn` is already enabled by the default filter (`info,ort=warn,...`), these messages already surface on stderr at the default log level — no `RUST_LOG` override needed. If you want to scope the firehose, `RUST_LOG=primer_pedagogy=warn` is the right target.

## Debugging a streaming hang

Symptom: the REPL waits indefinitely after sending a turn to the cloud or Ollama backend, no tokens printed, no error.

The streaming pipeline is a `futures::channel::mpsc::unbounded` driven by a tokio task that reads bytes from the HTTP response, frames them via a per-backend hand-rolled buffer — [`SseBuffer`](../../src/crates/primer-inference/src/cloud.rs) (Anthropic `event:`/`data:` framing), [`NdjsonBuffer`](../../src/crates/primer-inference/src/ollama.rs) (Ollama one-JSON-per-line), or `OpenAiSseBuffer` (the OpenAI `/v1/chat/completions` `data:` SSE format, terminated by `data: [DONE]`) — and forwards parsed events as chunks. A hang means one of three things:

1. **The HTTP request never completed handshake.** Check `RUST_LOG=hyper=debug,reqwest=debug`. If you see no `Sending request` line, the issue is config (URL, port, TLS).
2. **The response was 2xx but the body channel never delivered bytes.** This is the load-bearing case — Anthropic and Ollama can both legitimately keep a connection open with no data while the model warms up, but if it goes on past your retry budget, something is wrong server-side. Add a `tracing::trace!` inside `SseBuffer::push` / `NdjsonBuffer::push` to see whether bytes are arriving but failing to parse.
3. **Bytes are arriving but the parser is rejecting them.** This means the upstream protocol changed shape (e.g. a new SSE event variant Anthropic added). The parsers are hand-rolled by design — they accept the formats Phase 0.1 was built against — so a protocol drift surfaces here. Capture a raw byte dump (set tracing in the HTTP body loop, not the parser) and diff against the documented format.

> **Note:** Once `generate_stream` starts consuming bytes from a 2xx response, mid-stream errors propagate cleanly via the channel and the partial Primer turn is dropped at the dialogue-manager layer. Retries happen at the request-send + status-check phase only. If you find yourself wanting to retry mid-stream, that is a deeper design change — talk to maintainers first.

## Debugging a quiet classifier, extractor, or comprehension

Symptom: `--verbose` is on but the `[classifier]` (or `[extractor]` / `[comprehension]`) line never prints; the corresponding DB table is empty.

All three structured-output crates ([primer-classifier](../../src/crates/primer-classifier/src/lib.rs), [primer-extractor](../../src/crates/primer-extractor/src/lib.rs), [primer-comprehension](../../src/crates/primer-comprehension/src/lib.rs)) follow the same pattern: a tokio-spawned task runs after the response, persists to its DB table, and the dialogue manager `await`s the result with a bounded timeout at the start of the next turn. On timeout the task is **detached, not cancelled** — DB persistence still completes if the LLM eventually returns. So an empty DB table means the LLM call is failing, not just timing out.

The default timeouts are tuned for cloud latency: 3000 ms for the classifier, 5000 ms each for the extractor and comprehension classifier. Small local models behind Ollama routinely need more, especially when the extractor and comprehension run as a chained pair. Bump the relevant flag — `--classifier-timeout-ms`, `--extractor-timeout-ms`, `--comprehension-timeout-ms` — before assuming the integration is broken. The recipe below makes this concrete for the classifier; the same five-step shape works for the extractor and comprehension classifier with their respective flags.

### Recipe — Diagnose a quiet classifier

Symptoms: `--verbose` is on but no `[classifier]` line is printed; `turn_classifications` is empty for the current session.

1. **Confirm `--verbose` is set on the command line.** Without it, the line never prints even on success. The flag is on `primer-cli` — check `--help`.
2. **Look for the `[classifier]` line on stderr** after the most recent Primer response. The line is printed at the START of the next turn, after the spawned classifier task has been awaited (with a bounded timeout). If you only sent one turn, send a second one to trigger the await. If the line is still absent, the classifier task isn't completing within `--classifier-timeout-ms`.
3. **Inspect `turn_classifications` for the most recent session.**

   ```sql
   SELECT COUNT(*) FROM turn_classifications
   WHERE turn_id IN (
       SELECT id FROM turns
       WHERE session_id = (SELECT id FROM sessions ORDER BY started_at DESC LIMIT 1)
   );
   ```

   If 0, the classifier task either errored (`tracing::warn!` would have logged it — re-run with `RUST_LOG=primer_classifier=warn`) or is still pending in the background. The classifier task is detached on timeout; it self-persists when it eventually completes, so checking again a few seconds later may show the row.

4. **Increase `--classifier-timeout-ms`.** Default is 3000 ms. Small Ollama-served models (e.g. `gemma3:4b`, `llama3.2:3b`) often need 5000–8000 ms for the JSON-shaped classification call. The classifier is one model behind the chat — its result feeds turn N+1's intent decision, so a longer timeout adds at most that much latency to the *start* of turn N+1, not to the response on turn N.

5. **Swap to `--classifier-backend stub`** to confirm the integration point. The stub classifier writes a deterministic `EngagementAssessment` to `turn_classifications` instantly. If stub-backed runs produce rows and the `[classifier]` line, the LLM call itself is the bottleneck (revisit step 4 or check the model's chat-template support). If stub-backed runs *also* produce no rows, the integration is broken — start with `RUST_LOG=primer_pedagogy=debug,primer_classifier=debug` and read the spawn-then-await flow in [dialogue_manager/background.rs](../../src/crates/primer-pedagogy/src/dialogue_manager/background.rs).

The same recipe shape works for the extractor (use `--extractor-backend stub` and `--extractor-timeout-ms`, check `turn_concepts`) and the comprehension classifier (use `--comprehension-backend stub` and `--comprehension-timeout-ms`, check `turn_comprehensions`). Both lag one turn for in-memory state for the same reason the classifier does — see [chapter 6](06-classifiers-and-learner-model.md).
