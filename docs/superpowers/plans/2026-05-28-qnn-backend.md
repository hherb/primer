# Phase 1.2 — Qualcomm NPU Backend (QnnBackend) — Implementation Plan

**Date:** 2026-05-28
**Companion spec:** [docs/superpowers/specs/2026-05-28-qnn-backend-design.md](../specs/2026-05-28-qnn-backend-design.md)
**Target:** RedMagic 11 Pro (Snapdragon 8 Elite Gen 5)
**Starting model:** Qwen3-4B (pre-compiled on AI Hub, no HF gating)
**Branch:** `claude/stoic-davinci-ZKDpe` (developing); intend to split into 4-5 PRs as the steps below land.

## Task ordering

Each step is independently buildable, testable on desktop where possible, and lands behind a feature flag so default builds stay clean. The chain is mostly sequential — step N can't start until N-1 lands because each step depends on artefacts (the headers, the dlopen helper, etc.) from the previous.

---

## Step 1.2.0 — Pre-Primer validation (gate)

**Duration:** ~2 days. **PR:** none (validation only; results captured in a handoff doc).

Before writing any Primer code, prove the underlying pipeline works on the device. Failure here is the single most valuable signal: it shifts the entire phase's risk model.

### Tasks

1. Set up QAIRT SDK 2.29+ on a Linux/macOS dev box (Qualcomm developer portal sign-in). Capture the licence terms and the redistribution rules in `docs/devel/qairt-licence-notes.md`.
2. Use `qai-hub-models` to fetch the pre-compiled Qwen3-4B genie_bundle. Captured under `~/primer-bundles/qwen3-4b/`.
3. Clone `qualcomm/ai-hub-apps`, build `apps/chatapp_android` against the QAIRT SDK, sideload to the RedMagic 11 Pro. Confirm it answers a prompt with non-trivial text.
4. Capture: actual `tok/s` for decode phase, `tok/s` for prefill, time-to-first-token, peak `/sys/class/thermal` reading across a 5-minute interactive session. Write up to `docs/handoffs/2026-MM-DD-qnn-validation-chatapp.md`.
5. **Decision gate**: if measured decode rate < 8 tok/s on Qwen3-4B, **stop and reassess** — the Hexagon performance assumption is wrong and the rest of the plan needs revisiting (a smaller model, different quantisation, deferred phase).

### Deliverable

A handoff doc and a working `chatapp_android` install on the device with documented throughput numbers. No Primer code changes.

---

## Step 1.2.1 — `primer-qnn-sys` crate

**Duration:** ~2 days. **PR:** `feat(qnn): primer-qnn-sys crate with bindgen over Genie headers`.

### Tasks

1. `cargo new --lib src/crates/primer-qnn-sys`. Add to `src/Cargo.toml` workspace `members`.
2. Vendor headers under `src/crates/primer-qnn-sys/headers/` from `$QAIRT_SDK_ROOT/include/Genie/`. Include the licence file from QAIRT alongside, named clearly (`HEADERS_LICENCE.txt`).
3. Write `build.rs` that runs `bindgen` against the vendored headers, outputs `bindings.rs` to `OUT_DIR`. `bindgen` is a dev-dependency only; the generated bindings are in `OUT_DIR`, not committed.
4. `src/lib.rs` re-exports the generated bindings, plus a small `GenieLibrary` struct that wraps `libloading::Library` and lazy-resolves the four functions the binding needs (`GenieDialogConfig_createFromJson`, `GenieDialogConfig_free`, `GenieDialog_create`, `GenieDialog_setTokenCallback`, `GenieDialog_query`, `GenieDialog_free`).
5. On non-Android targets, `GenieLibrary::open` returns `Err("QNN is Android-only")` — keeps `cargo check` green from desktop.
6. Add a trivial unit test that asserts opening fails on the host (sanity).

### Tests

Pure host-side. The crate is too thin to warrant integration tests at this step.

### Verification

```bash
cd src
~/.cargo/bin/cargo build -p primer-qnn-sys
~/.cargo/bin/cargo test -p primer-qnn-sys
~/.cargo/bin/cargo clippy -p primer-qnn-sys --all-targets
```

All green on macOS / Linux / Termux.

---

## Step 1.2.2 — `QnnBackend` skeleton, non-streaming

**Duration:** ~2 days. **PR:** `feat(qnn): QnnBackend with non-streaming generate()`.

### Tasks

1. Add `primer-qnn-sys` as a dep of `primer-inference`, behind a new `qnn` cargo feature on `primer-inference`.
2. New module `primer-inference/src/qnn.rs`. Define `QnnBackend`, `GenieDialogSession`, the safety wrappers.
3. `QnnBackend::new(bundle_dir, qairt_lib_dir)`: dlopen the library, validate bundle dir contents, create the dialog handle. All inside `tokio::task::spawn_blocking` (Genie creation blocks).
4. Implement `InferenceBackend` for `QnnBackend`:
   - `name()`: returns `format!("qnn:{}", model_id)` from cached `primer-meta.json`.
   - `is_available()`: returns true if construction succeeded.
   - `generate_stream()`: **for this step, implement via collecting the synchronous query result into a single TokenChunk{done:true} stream**. Streaming bridge lands in 1.2.3.
5. Chat-template render: read `primer-meta.json::chat_template` (Jinja2 string), substitute system+messages. Decide between `minijinja` and hand-rolled (see open question §12.2 of the spec). Prefer `minijinja` if it's already in the tree.
6. `primer-meta.json` parser as a small module `qnn::meta`.
7. New `Drop` impl that calls `GenieDialog_free` + `GenieDialogConfig_free`.

### Tests

- Unit: `primer-meta.json` round-trip. Chat-template substitution for ChatML and Llama-3-Instruct shapes. Construction error paths (missing bundle, missing libs) produce the right error variants.
- Mock: introduce `GenieLibraryHandle` as a small trait (`open_dialog_from_config`, `query`, `set_token_callback`) so a `MockGenieLibrary` can stub out the C calls. Test the construction → generate → drop happy path.

### Verification

```bash
cd src
~/.cargo/bin/cargo build -p primer-inference --features qnn
~/.cargo/bin/cargo test -p primer-inference --features qnn
```

---

## Step 1.2.3 — Streaming token callback

**Duration:** ~1 day. **PR:** `feat(qnn): streaming token bridge via mpsc + C-ABI callback`.

### Tasks

1. Replace the single-shot `generate_stream` from 1.2.2 with the C-ABI callback pattern:
   - `extern "C" fn on_token(...)` that forwards into a `futures::channel::mpsc::UnboundedSender<Result<TokenChunk>>`.
   - `Box::into_raw` the sender to get a `*const c_void` for `user_data`.
   - Inside `tokio::task::spawn_blocking`: set callback, call `GenieDialog_query`, on return send the final `done: true` chunk and `Box::from_raw` the sender (close the channel).
2. Return the receiver wrapped in `Pin<Box<dyn Stream<Item = Result<TokenChunk>> + Send>>` exactly matching the existing `TokenStream` alias.
3. Carry a `tokio::sync::Mutex<GenieDialogSession>` so concurrent `generate_stream` calls serialize. Document the rationale (Genie is single-session-per-dialog).
4. Mid-stream error path: a callback that receives a non-OK Genie status sends `Err(PrimerError::Inference(...))` on the channel; existing `DialogueManager` error handling drops the partial turn.

### Tests

- Mock streaming: extend `MockGenieLibrary` to invoke the callback N times before returning. Assert the receiver yields N+1 chunks (N body + 1 done).
- Mid-stream error: mock callback that emits 2 chunks then signals an error. Assert receiver yields 2 Ok + 1 Err and closes.
- Mutex serialization: two concurrent `generate_stream` calls; assert they complete in order, not interleaved.

### Verification

Same as 1.2.2. All tests pass on host (mock). Real-device validation happens in 1.2.6.

---

## Step 1.2.4 — CLI wiring

**Duration:** ~0.5 day. **PR:** can be folded into 1.2.5 or 1.2.6 if small.

### Tasks

1. Add `qnn` feature on `primer-cli`, pulling `primer-inference/qnn` and `primer-engine/qnn`.
2. New CLI flag `--backend qnn` (extend the enum in `primer-cli/src/main.rs`).
3. New CLI flag `--qnn-bundle-dir <path>` (clap `requires("backend", "qnn")`).
4. New CLI flag `--qnn-qairt-lib-dir <path>` (optional; defaults to `bundle_dir.parent().join("qairt/lib/aarch64-android/")`).
5. Env-var fallbacks: `PRIMER_QNN_BUNDLE_DIR`, `PRIMER_QNN_QAIRT_LIB_DIR`.
6. `primer-engine`: extend the backend-construction helper to handle the QNN variant. Mirror existing Ollama/OpenAI-compat patterns.
7. **Startup validation under `--backend qnn`**: if classifier/extractor/comprehension backends are all `qnn`, warn that this serializes all NPU work; if all are `stub`, warn the conversation will lose classifier-driven features; if cloud-backed and `ANTHROPIC_API_KEY` missing, hard-error.

### Tests

- CLI parse: `--backend qnn` requires `--qnn-bundle-dir`. Without, clap rejects.
- CLI parse: `--no-persist` + `--backend qnn` still works (existing matrix).
- Engine wiring: a unit test that `build_backend_for_config` returns a `QnnBackend` when the config selects it.

### Verification

```bash
cd src
~/.cargo/bin/cargo build --bin primer --features qnn
~/.cargo/bin/cargo test -p primer-cli --features qnn
~/.cargo/bin/cargo run --bin primer --features qnn -- --help | grep -i qnn
```

---

## Step 1.2.5 — Context budget tuning under 4K

**Duration:** ~1 day. **PR:** `feat(pedagogy): per-backend context window budget`.

### Tasks

1. New field on `PedagogyConfig`: `context_window_turns_qnn: Option<usize>` (default `Some(12)`). When set and the active backend's `name()` starts with `"qnn:"`, the dialogue manager uses this value instead of the global `context_window_turns`.
2. New field on `RetrievalParams`: `kb_top_k_qnn: Option<usize>` (default `Some(3)`), mirroring the same per-backend pattern.
3. Document both in `consts.rs` as Phase 1.2 additions.
4. **Important**: name the fields backend-class-by-class, not Qualcomm-specifically. A future 4K-bound non-Qualcomm backend gets the same treatment without rename.

Actually — reconsidering — naming by *backend* leaks an implementation detail across crate boundaries. Better: add `context_capacity_tokens: u32` to `primer-meta.json` and have the dialogue manager auto-budget based on capacity vs prompt-token estimate. **Refine this during step 1.2.5 implementation**; the spec leaves the option open.

### Tests

- Dialogue manager unit: with a backend whose name starts with `"qnn:"`, recent-turn window is 12 not 20.
- Retrieval: KB retrieval respects `kb_top_k_qnn` when set.

---

## Step 1.2.6 — Benchmark + thermal harness

**Duration:** ~2 days. **PR:** `feat(qnn): benchmark example and thermal capture`.

### Tasks

1. New file `data/bench/socratic_prompts.jsonl`: 30 representative dialogue-continuation prompts drawn from `tests/common/en.rs::QUERIES` plus seeded continuations capturing the "child responds, Primer responds, child reacts" shape.
2. New `primer-inference/examples/qnn_bench.rs`:
   - CLI args: `--bundle-dir`, `--qairt-lib-dir`, `--prompts`, `--duration-secs`, `--thermal-out`.
   - Construct `QnnBackend`. For each prompt: measure TTFT, decode tok/s. Repeat until `duration-secs` elapsed.
   - In a background tokio task, sample `/sys/class/thermal/thermal_zone*/temp` every 2s, write CSV.
   - Final report: p50/p95 TTFT, mean/min decode tok/s, peak thermal. Pass/fail vs targets (15 tok/s decode, <3s TTFT, <70°C).
3. Update `docs/devel/redmagic-termux-quickstart.md`:
   - QAIRT install section.
   - The `--backend qnn` row added to the "What works, what doesn't" table.
   - A "Run the benchmark" section pointing at `qnn_bench`.

### Tests

The bench harness itself doesn't have unit tests beyond CLI-parse sanity. It is the device test.

### Verification

```bash
# On RedMagic 11 Pro via Termux:
cd ~/primer/src
~/.cargo/bin/cargo run --release --example qnn_bench --features qnn -- \
    --bundle-dir ~/primer-bundles/qwen3-4b \
    --duration-secs 900 \
    --thermal-out ~/storage/shared/primer-thermal.csv

# Pass: decode >= 15 tok/s, TTFT < 3s, peak temp < 70°C.
```

The benchmark numbers and the thermal CSV get attached to the closing handoff doc.

---

## Cross-cutting work

Items that aren't one step but live across them:

- **CI updates**: Add `cargo build --bin primer --features qnn` to the Android cross-compile workflow at `.github/workflows/ci.yml`. (Note: it won't *link* — QAIRT `.so`s aren't in CI — but it should *compile* under `cargo build --no-run` or equivalent.) Use `cargo check` if needed.
- **CLAUDE.md updates**: After step 1.2.6 lands, the architecture section gets a paragraph on `QnnBackend`, the `primer-qnn-sys` crate, and the QAIRT bundle pattern. Same "gotchas" section gets a row about the QAIRT install step and the 4K context budget.
- **Issue tracker**: file follow-ups for (a) reasoning-mode token stripping across Ollama/OpenAI-compat/QNN, (b) classifier-on-NPU as a secondary session, (c) Llama-3.x export via `qai-hub-models` once Meta gating is sorted, (d) auto-detection / picker UI for multiple bundle dirs.

## Risk register (live)

| Risk | Mitigation | Trigger |
|---|---|---|
| QAIRT won't load on Termux's sandboxed user | Validated explicitly in 1.2.0 before any Primer code | If 1.2.0 fails: package the Primer as a Tauri-Android app instead of Termux CLI, pushing Phase 1.2 into Phase 3 enclosure scope |
| Decode tok/s below 15 on Qwen3-4B | Try Llama-3.2-3B (smaller); try 8-bit-activations variant if available | If 1.2.0 measures < 8 tok/s |
| 4K context too tight even with budget tuning | Re-export at 8K (memory cost) or aggressively shrink the system prompt | If 1.2.6 shows in-conversation context overruns |
| Qualcomm licence forbids redistribution | Don't redistribute the .so; document the user-side install step instead | If §5 legal pass flags it |
| `bindgen` produces unusable bindings for some Genie types | Hand-roll the function declarations (only ~6 functions needed) | If 1.2.1 hits incompatibilities |

## Done definition

Phase 1.2 is closed when **all** of the following are true:

- `cargo build --bin primer --features qnn` succeeds on a Termux on RedMagic 11 Pro.
- Running with `--backend qnn` yields a working Socratic conversation with Qwen3-4B.
- Measured TTFT < 3s, sustained decode >= 15 tok/s, peak thermal <= 70°C.
- 850+ workspace tests still pass with no regressions.
- `docs/devel/redmagic-termux-quickstart.md` reflects the new `--backend qnn` path.
- A handoff doc captures the empirical numbers and remaining follow-ups.
- ROADMAP.md marks Phase 1.2 ✅ with a pointer to this plan and the closing handoff.
