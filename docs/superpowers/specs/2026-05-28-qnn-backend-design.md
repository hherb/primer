# Qualcomm NPU Inference Backend (QnnBackend) — Design

**Date:** 2026-05-28
**Status:** Design
**Roadmap slot:** Phase 1.2 — Qualcomm NPU (RedMagic / Snapdragon 8 Elite)
**Target device:** RedMagic 11 Pro (Snapdragon 8 Elite Gen 5, Hexagon v79-class, 45 TOPS, 24 GB RAM)
**Companion plan:** [docs/superpowers/plans/2026-05-28-qnn-backend.md](../plans/2026-05-28-qnn-backend.md)

## Motivation

The Phase 0 RedMagic 11 Pro validation (`docs/devel/redmagic-termux-quickstart.md`, 2026-05-26) confirmed empirically what the inference architecture doc predicted: **CPU-only inference at 4B Q4 on Snapdragon 8 Elite is too slow for conversational Socratic dialogue**. The phone is fully usable in cloud mode but the standalone-phone product story — running the Primer offline on a child's device — is blocked until NPU acceleration lands. That is this work.

Qualcomm's Genie SDK (on top of QAIRT/QNN) is the canonical path from a HuggingFace LLM to a Hexagon NPU. The reference Android consumer is `apps/chatapp_android` in `qualcomm/ai-hub-apps` (BSD-3). The trait surface in `primer-core::InferenceBackend` is identical-shape to what Genie's C API needs: construct → set token callback → call query → stream tokens. The `QnnBackend` slot in `primer-inference/src/lib.rs` has been a documented TODO since Phase 0.

## Scope

1. **`primer-qnn-sys`** — new crate exposing minimal unsafe bindings over the Genie C API via `bindgen`.
2. **`primer-inference::QnnBackend`** — new `InferenceBackend` impl driving Genie from Rust, with streaming token callbacks bridged into the existing `TokenStream` shape.
3. **Bundled QAIRT runtime** — ship `libGenie.so` + `libQnnHtp.so` + the supporting Hexagon DSP skel libraries inside the Primer install tree under `~/primer/qairt/lib/aarch64-android/`, dlopened lazily at backend construction.
4. **CLI wiring** — `--backend qnn --qnn-bundle-dir <path>` (plus `PRIMER_QNN_BUNDLE_DIR` env fallback), behind a `primer-cli/qnn` cargo feature so default desktop builds stay light.
5. **Classifier routing decision** — when `--backend qnn` is active, default the classifier/extractor/comprehension chain to cloud (the two-tier model from `inference_architecture.md`). Document the override path.
6. **Benchmark harness** — capture time-to-first-token, sustained tokens/sec, peak thermal reading across a 15-minute Socratic dialogue, comparable to the cloud baseline.

## Non-goals

- **No Tauri-Android GUI wrapper.** Phase 1.2 is CLI-only on Termux. GUI on the phone is a Phase 3 enclosure concern.
- **No Llama / Phi / Gemma support in this phase.** Qwen3-4B only; other models are a follow-up. Llama-3.x specifically requires Meta HuggingFace gate signup and a local `qai-hub-models` export.
- **No reasoning-mode models.** DeepSeek-R1, Qwen3-Thinking, Phi-4-reasoning leak `<think>…</think>` tokens to the child verbatim, same gotcha as `OllamaBackend`. Stream-aware reasoning-marker stripping is a separate cross-backend issue (file as a follow-up).
- **No hybrid retrieval on this device.** The `ort-sys` Android blocker is orthogonal; BM25-only retrieval already passes 100% strict recall on the 91-query English benchmark. Hybrid is a separate workstream.
- **No second NPU session for classifiers.** RAM headroom on a 4B-with-classifier-on-NPU build is too tight to be the default; cloud-classifier is the documented routing.

---

## 1. Genie SDK surface (recap of upstream)

The Genie C API (from QAIRT 2.29+) is roughly:

```c
GenieDialogConfig_Handle_t cfg = NULL;
GenieDialog_Handle_t       dlg = NULL;

GenieDialogConfig_createFromJson(genie_config_json_path, &cfg);
GenieDialog_create(cfg, &dlg);
GenieDialog_setTokenCallback(dlg, on_token, user_data);
GenieDialog_query(dlg, prompt_text, GENIE_DIALOG_SENTENCE_COMPLETE, NULL);
GenieDialog_free(dlg);
GenieDialogConfig_free(cfg);
```

The `genie_config.json` references the per-shard context binaries (`weight_sharing_model_N_of_K.serialized.bin`) and `tokenizer.json` by relative path. The token callback is C-ABI synchronous and fires once per token (or token-chunk depending on the exporter). `GenieDialog_query` blocks the calling thread until end-of-generation.

There are several Genie features the Primer does not need today and the v1 binding will skip: profiling hooks, custom samplers, save/restore of dialog state, batched query. The trait surface in `InferenceBackend` covers exactly `generate_stream`; nothing more.

## 2. Crate: `primer-qnn-sys`

A new workspace crate at `src/crates/primer-qnn-sys/`. Internals only — never depended on outside `primer-inference`. Pinned to `target_os = "android"` for runtime; on desktop hosts it compiles to a stub so `cargo check` works for editors and CI from any machine.

### Layout

```
primer-qnn-sys/
├── Cargo.toml
├── build.rs            # bindgen over Genie headers, links libGenie via dlopen
├── headers/            # Vendored GenieDialog.h, GenieDialogConfig.h, GenieCommon.h
└── src/
    └── lib.rs          # unsafe extern "C" function declarations + opaque types
```

`build.rs` runs `bindgen` against the vendored headers, generates `bindings.rs`, and emits `cargo:rerun-if-changed` for the headers. The link strategy is `libloading::Library::new("libGenie.so")` at runtime (NOT `#[link(name = "Genie")]`) — see §5 for why.

### Public surface

Just the raw `extern "C" fn` declarations matching the Genie headers, the opaque handle structs, and the `Genie_Status_t` enum. No safety wrappers — those live in `primer-inference`. Compiles on desktop targets via a stub that produces a `Library::open` that returns `Err`, so `is_available()` correctly reports `false` everywhere except Android.

### Feature flags

```toml
[features]
default = []
```

No features. The crate is always compiled when included; consumers gate via target/feature flags higher up.

## 3. Crate: `primer-inference::qnn`

### Struct

```rust
pub struct QnnBackend {
    /// Per-process Genie library handle (libGenie.so via dlopen).
    lib: Arc<GenieLibrary>,
    /// Path to the genie_bundle directory (contains genie_config.json + .bin shards).
    bundle_dir: PathBuf,
    /// Human-readable backend name including model id, e.g. "qnn:qwen3-4b".
    name: String,
    /// Tokio mutex serializing access to the single Genie dialog session.
    /// Genie does not support concurrent queries on the same dialog handle.
    dialog: Arc<tokio::sync::Mutex<GenieDialogSession>>,
}

struct GenieDialogSession {
    cfg_handle: GenieDialogConfigHandle,
    dialog_handle: GenieDialogHandle,
}
```

The dialog handle is constructed once at backend startup (expensive — model weights map to NPU) and reused across every `generate_stream` call. A `tokio::sync::Mutex` serializes concurrent callers; in practice the dialogue manager calls one inference at a time, and the classifier chain routes elsewhere by default (§6).

### Construction

```rust
pub async fn new(bundle_dir: PathBuf, qairt_lib_dir: PathBuf) -> Result<Self> { ... }
```

`qairt_lib_dir` defaults to `bundle_dir.parent().join("qairt/lib/aarch64-android/")` when not passed explicitly. The constructor:

1. `dlopen` `libGenie.so` and its transitive dependencies (`libQnnHtp.so`, etc.) from `qairt_lib_dir`. Fails fast with `PrimerError::Inference(...)` and a clear hint if the library is missing.
2. Validates `bundle_dir` contains `genie_config.json` and the `.bin` shards referenced by it.
3. Creates the `GenieDialogConfig` from JSON, then the `GenieDialog`. Failures here are `PrimerError::Inference(InferenceError::Other(...))` — the user gets a generic "the local model didn't load, falling back to cloud" message via the i18n boundary.
4. Reads model id from a sibling `primer-meta.json` (Primer-authored, not Qualcomm-shipped) for the `name()` string. If absent, derives from `bundle_dir.file_name()`.

### `InferenceBackend` impl

| Method | Behaviour |
|--------|-----------|
| `name()` | `"qnn:<model-id>"`, e.g. `"qnn:qwen3-4b"` |
| `is_available()` | `true` if `lib` and `dialog` constructed successfully. No further probe (Genie has no health-check call). |
| `generate_stream()` | Serializes prompt to a single string via the model's chat template, calls Genie with streaming callback, surfaces tokens through `futures::channel::mpsc::unbounded` |

### `generate_stream` flow

```text
1. Acquire dialog mutex.
2. Render prompt (system + messages) to a single template string using
   per-model chat template (Qwen3 uses ChatML <|im_start|>/<|im_end|>;
   see §4 for the chat-template strategy).
3. Create mpsc::unbounded<Result<TokenChunk>> pair.
4. Box the Sender, get a raw pointer for use_data.
5. tokio::task::spawn_blocking:
     - inside: set token callback (extern "C" fn forwards through ptr → tx)
     - inside: call GenieDialog_query (blocks)
     - on return: send final TokenChunk{done: true}
     - on error: send Err(PrimerError::Inference(...))
6. Return the Receiver wrapped in Pin<Box<dyn Stream>>.
```

The C-ABI token callback:

```rust
extern "C" fn on_token(
    response_str: *const c_char,
    response_role: GenieDialog_SentenceCode_t,
    user_data: *const c_void,
) {
    // SAFETY: user_data is a Box<UnboundedSender<...>> leaked across FFI.
    //          Lifetime is held by the spawn_blocking until completion.
    let tx = unsafe { &*(user_data as *const UnboundedSender<Result<TokenChunk>>) };
    let text = unsafe { CStr::from_ptr(response_str) }.to_string_lossy().into_owned();
    let _ = tx.unbounded_send(Ok(TokenChunk { text, done: false }));
}
```

Mid-generation error from the callback (e.g. NPU returned a non-zero status) propagates by sending `Err(...)` on the same channel. The dialogue manager's existing mid-stream-error handling (drop the partial Primer turn, surface the error) applies unchanged.

### Drop

`Drop for QnnBackend` releases the dialog and config handles via `Genie_*_free`. The library handle is `Arc`-shared and freed when the last `QnnBackend` instance drops.

## 4. Chat template strategy

Genie's `GenieDialog_query` accepts a single rendered prompt string — there is no message-list API. The Primer's `Prompt { system, messages }` shape must be flattened to the chat template the exported model expects.

Two options:

- **(a)** Hard-code per-model templates in Rust, gated on model id (`"qwen3"` → ChatML, `"llama-3"` → Llama-3-Instruct format, etc.).
- **(b)** Read a `chat_template` field from `primer-meta.json` alongside the genie_bundle, where the model exporter records the correct format.

**Decision: (b), with a small built-in fallback table.** The exporter knows; the runtime shouldn't have to. The fallback table (ChatML for unknown models) keeps things working in development. The `chat_template` field is a Jinja2-shape string parsed by `minijinja` (already used elsewhere in the workspace? — verify; if not, this becomes a new dep with a small footprint).

## 5. QAIRT runtime distribution

QAIRT 2.29+ ships from Qualcomm developer portal as a tarball. The Android `.so` files relevant to Genie are:

- `libGenie.so` — the high-level dialog API
- `libQnnHtp.so` — Hexagon Tensor Processor backend
- `libQnnHtpV79Skel.so` — Hexagon DSP skel for v79 architecture (matches 8 Elite Gen 5)
- `libQnnHtpPrepare.so`, `libQnnSystem.so` — supporting libs

Bundle plan:

```text
~/primer/qairt/lib/aarch64-android/
├── libGenie.so
├── libQnnHtp.so
├── libQnnHtpV79Skel.so
├── libQnnHtpPrepare.so
├── libQnnSystem.so
└── LICENSE.qualcomm       ← Qualcomm's redist licence text, surfaced in the consent screen
```

These are **not** committed to the repo. The quickstart adds a step:

```bash
# After installing QAIRT 2.29 from developer.qualcomm.com:
mkdir -p ~/primer/qairt/lib/aarch64-android/
cp $QAIRT_SDK_ROOT/lib/aarch64-android/{libGenie.so,libQnnHtp.so,...} \
   ~/primer/qairt/lib/aarch64-android/
```

A future `primer-qnn-fetch` helper could automate this for users with portal credentials, but v1 is manual. The `QnnBackend::new` constructor `dlopen`s these libraries from the configurable `qairt_lib_dir` and fails loudly with the install-step pointer if any is missing.

### Licence note

QAIRT is closed-source Qualcomm proprietary; the runtime libs are "system library" in shape. AGPL-with-system-library is a well-trodden pattern (cf. AGPL apps that link CUDA). The user-facing licence screen the Primer surfaces at first NPU use must include Qualcomm's redist text. A short pre-commit pass with legal counsel is warranted before this ships in any public release; the design choice here is to keep the QAIRT path **strictly opt-in** (a cargo feature, a CLI flag, a deliberate install step) so a user who has not consented never loads the library.

## 6. Classifier routing under `--backend qnn`

Per `inference_architecture.md`, the two-tier model has a local primary backend and a cloud supervisor. The classifier/extractor/comprehension chain is naturally a "supervisor" workload:

- **Latency-insensitive**: runs in the inter-turn gap, current `extractor_settings.blocking_timeout = 5000ms` already absorbs cloud-API latency.
- **Quality-sensitive**: a well-tuned cloud model classifies more reliably than a 4B local model.
- **Resource-conflicting on a phone**: serializing on the NPU adds 2-3 chained calls to each turn boundary; opening a second Genie session blows the RAM budget on a 24 GB device once the main 4B model is loaded.

**Default routing under `--backend qnn`:**

| Component | Default backend | Override |
|---|---|---|
| Main dialogue | `qnn` | n/a |
| Engagement classifier | `cloud` (Anthropic) | `--classifier-backend qnn` (single-session serialized) |
| Concept extractor | `cloud` | `--extractor-backend qnn` |
| Comprehension classifier | `cloud` | `--comprehension-backend qnn` |
| Embedder | `none` (BM25-only) | unchanged — `ort-sys` Android blocker is orthogonal |

This means **`--backend qnn` implies `ANTHROPIC_API_KEY` is present** unless the user explicitly stubs the classifiers. The CLI validates this at startup and prints a clear hint if both are missing. A full-offline `--backend qnn` config requires either passing `qnn` to every classifier flag or accepting `stub` classifiers (engagement state falls back to the word-count heuristic; comprehension/extractor become no-ops).

The docs are explicit about this: "Phase 1.2 makes the *main dialogue* offline-capable. Full classifier offline-capability is a follow-up that needs either a second NPU session, a CPU-based small model, or quantisation work to fit a 1B-class classifier model in the residual RAM."

## 7. Bundle staging and discovery

A genie_bundle is a directory containing:

```text
~/primer/models/qwen3-4b/
├── genie_config.json                          ← references the bins by relative path
├── tokenizer.json
├── weight_sharing_model_1_of_4.serialized.bin
├── weight_sharing_model_2_of_4.serialized.bin
├── weight_sharing_model_3_of_4.serialized.bin
├── weight_sharing_model_4_of_4.serialized.bin
├── htp_backend_ext_config.json                ← perf knobs (sustained-perf vs burst)
└── primer-meta.json                           ← NEW; Primer-authored
```

`primer-meta.json` is a small Primer-owned sidecar (not Qualcomm-shipped):

```json
{
  "model_id": "qwen3-4b",
  "context_length": 4096,
  "chat_template": "<|im_start|>{{role}}\n{{content}}<|im_end|>\n",
  "vocab_size": 151936,
  "stop_sequences": ["<|im_end|>", "<|endoftext|>"]
}
```

This is what the Rust side reads. Adding a new model = drop a bundle dir and a meta file; no code change.

CLI flag: `--qnn-bundle-dir <path>` (or env `PRIMER_QNN_BUNDLE_DIR`). No auto-discovery in v1 — the user points at one bundle. (A future v2 could scan `~/primer/models/` and offer a picker.)

## 8. Context window budget under 4K

Qwen3-4B genie_bundles default to a 4096-token context. The Primer's prompt today bundles:

| Section | Typical token budget |
|---|---|
| System instructions (Socratic guidance, age calibration, locale pack) | ~400-600 |
| Rolling session summary | ~150 |
| Retrieved older-turns (FTS, up to 5) | ~250 |
| Retrieved KB passages (BM25/hybrid, up to 5) | ~600-1000 |
| Due-vocab section | ~50-100 |
| Recent-turn window (default 20 turns) | ~1000-2000 |
| **Total before generation** | **~2500-4000** |

This is **tight at 4K**. Two mitigations land in this phase:

1. **`context_window_turns` per-backend tunable**: a new `PedagogyConfig.context_window_turns_qnn` defaulting to 12 (vs the global 20). When `--backend qnn`, the dialogue manager truncates harder.
2. **A KB-passage budget for QNN**: cap retrieved KB at 3 passages × ~150 tokens each = ~450 tokens (vs current 5 × ~150 = 750). Behind a `retrieval_kb_top_k_qnn` setting.

Both are documented; neither is intrinsic to QNN — a user on a non-Qualcomm 4K-context model would want the same. The naming acknowledges QNN is just the first 4K-bound backend.

Stretch goal: export Qwen3-4B at 8K context (Genie supports this with a memory cost) once the 4K path is proven. Out of scope for this phase.

## 9. Benchmark + thermal harness

A new `examples/qnn_bench.rs` in `primer-inference`:

```bash
cargo run --release --example qnn_bench --features qnn -- \
    --bundle-dir ~/primer/models/qwen3-4b \
    --prompts data/bench/socratic_prompts.jsonl \
    --duration-secs 900 \
    --thermal-out /sdcard/primer-thermal.csv
```

Output:

- Time-to-first-token (p50, p95) over N prompts
- Sustained tokens/sec (decode phase) over 15-minute conversation simulation
- `/sys/class/thermal/thermal_zone*/temp` sampled every 2s, written to CSV
- Final report: pass/fail against `>=15 tok/s decode`, `<3s TTFT`, `<70°C sustained` targets

`data/bench/socratic_prompts.jsonl` is a new file: 30 representative Socratic-dialogue prompts plucked from the existing benchmark corpora (`tests/common/en.rs` queries + a few seeded continuations).

## 10. Failure modes and graceful degradation

| Failure | Behaviour |
|---|---|
| `libGenie.so` not found at `qairt_lib_dir` | Hard error at startup with the install-step pointer. CLI suggests `--backend cloud` fallback. |
| `genie_config.json` references missing shard `.bin` | Hard error at startup. |
| Model load succeeds but first query returns `GENIE_STATUS_ERROR_*` | Surface as `PrimerError::Inference(InferenceError::Other("..."))`, log via `tracing::warn!`. The dialogue manager already drops the partial turn cleanly. |
| Mid-generation NPU thermal throttle | Out of our hands — Genie surfaces the slowdown as a slower tok/s, not a status error. The harness measures this. |
| Token callback fires after `GenieDialog_query` returned (shouldn't happen but defend) | Drop on the floor (mpsc receiver is closed). |
| Cloud classifier fails while NPU main runs | Already handled — classifier soft-fails return `EngagementAssessment::unknown_low_confidence` and `tracing::warn!`, the conversation continues. |
| Reasoning-mode model picked accidentally | `<think>` tags leak into response. Documented gotcha; not blocked at this layer. |

The single load-bearing graceful-degradation path is **cloud fallback when NPU not available**: a future `InferenceRouter` (also in `inference_architecture.md` §"Inference Router Design") could detect QnnBackend's `is_available() == false` at startup and silently use cloud. For v1 the user picks the backend explicitly; the router design is Phase 1.3.

## 11. Testing strategy

Three layers:

- **Unit tests (cross-platform, no NPU required)**: prompt template rendering, `primer-meta.json` parsing, chat-template substitution, dlopen-failure error messages. These pin contracts that desktop CI can verify.
- **Mocked integration tests**: A `MockGenieLibrary` that satisfies the `GenieLibrary` trait (small enough that it's a worthwhile abstraction) and returns canned tokens. Exercises the streaming bridge, the C-ABI callback safety, the mutex serialization, the mid-stream error path. Runs in default CI.
- **Device tests (Android, manual)**: The `examples/qnn_bench.rs` harness. Documented in the quickstart with a "run this once after install" step. Not on CI yet (no Android NPU runner).

Out of scope for v1: a no-op `QnnBackend::new` on desktop that returns `is_available() == false` so a unified config can ship across desktop and Android. Useful, but the CLI feature-flag pattern (`primer-cli/qnn`) achieves the same shape today.

## 12. Open questions

1. **Does QAIRT 2.29 actually load on Termux's sandboxed Android user**, or does it require app-context paths the OEM controls? Step 1.2.0 of the plan validates this *before* any Primer code is written.
2. **Is `minijinja` already in the workspace?** If yes, reuse for the chat template. If no, evaluate cost (~150 KB binary, MIT licence) vs hand-rolling the 4 chat templates Qwen3/Llama-3/ChatML/Mistral need.
3. **Should `qairt/lib/` live under `~/primer/qairt/` or under the binary directory?** The former survives `cargo install --path`; the latter is easier to wipe. Tentatively the former; the install step is one-time anyway.
4. **What happens to `--features primer-cli/speech` + `--features primer-cli/qnn` on Termux?** Both pull `ort-rc.10` (speech) and `dlopen` (qnn). They should not conflict. Sanity-check during step 1.2.1.

## 13. Success criteria

- [ ] `cargo run --bin primer --features qnn -- --backend qnn --qnn-bundle-dir ~/primer/models/qwen3-4b --name Aiyana --age 8` works on the RedMagic 11 Pro inside Termux.
- [ ] First token arrives in **< 3 seconds**.
- [ ] Sustained decode rate **>= 15 tok/s** across a 15-minute Socratic conversation.
- [ ] Peak thermal **<= 70°C** at sustained load (the phone's published throttling point).
- [ ] Classifier chain routes to cloud by default; no NPU-related interference with engagement / extractor / comprehension behaviour.
- [ ] All existing 850+ workspace tests continue to pass (no regression on default desktop builds).
- [ ] Quickstart at `docs/devel/redmagic-termux-quickstart.md` updated with the QAIRT install step and the new `What works, what doesn't` row showing `--backend qnn` ✅.
