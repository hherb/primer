# OpenAI-Compatible Inference & Embedding Backend

**Date:** 2026-05-15
**Status:** Design
**Motivation:** oMLX, LM Studio, vLLM, llama.cpp `--server`, and dozens of other local/remote inference servers expose the OpenAI chat-completions API. Today the Primer can talk to Ollama (proprietary `/api/chat` NDJSON format) or Anthropic Cloud (proprietary SSE format), but not to any OpenAI-compatible server. Adding a generic OpenAI-compatible backend unlocks ~20-40% throughput gains on Apple Silicon via oMLX/MLX-native servers, plus access to remote providers (Together, Groq, OpenRouter) that use the same API shape — all with no architectural changes.

## Scope

1. **`OpenAiCompatBackend`** — new `InferenceBackend` impl in `primer-inference` that speaks `/v1/chat/completions` with SSE streaming.
2. **`OpenAiCompatEmbedder`** — new `Embedder` impl in `primer-embedding` that speaks `/v1/embeddings` with native batching.
3. **CLI + engine wiring** — `--backend openai-compat` and `--embedder-backend openai-compat` flags with associated URL/key/model options.

## Non-goals

- Replacing the Ollama backend (Ollama has its own proprietary API; users who prefer Ollama keep using `--backend ollama`).
- Tool-use / function-calling support (the Primer doesn't use OpenAI tool calls).
- Vision / multimodal input (text-only for now).
- GUI wiring (follows separately once the CLI backend is proven).

---

## 1. Inference backend: `primer-inference/src/openai_compat.rs`

### 1.1 Struct

```rust
pub struct OpenAiCompatBackend {
    client: reqwest::Client,
    base_url: String,        // e.g. "http://localhost:8000" or "https://api.together.xyz"
    model: String,           // e.g. "mlx-community/Qwen3-8B-4bit"
    api_key: Option<String>, // None for local servers, Some for remote providers
    retry_settings: primer_core::retry::RetrySettings,
}
```

Construction: `OpenAiCompatBackend::new(base_url, model, api_key)`.

### 1.2 `InferenceBackend` impl

| Method | Behaviour |
|--------|-----------|
| `name()` | `"openai-compat"` |
| `is_available()` | `GET {base_url}/v1/models` returns 2xx. Gracefully `false` on any error. |
| `generate_stream()` | `POST {base_url}/v1/chat/completions` with `stream: true`. See SSE parsing below. |

`generate()` inherits the default trait impl (collects from stream).

### 1.3 Request shape

```json
{
  "model": "<model>",
  "messages": [
    {"role": "system", "content": "<system prompt>"},
    {"role": "user", "content": "..."},
    {"role": "assistant", "content": "..."}
  ],
  "stream": true,
  "temperature": 0.7,
  "top_p": 0.95,
  "max_tokens": 1024,
  "stop": ["<stop_seq>"]
}
```

System prompt is a `"system"` role message (same as Ollama; unlike Anthropic's top-level field).

If `api_key` is `Some(key)`, the request carries `Authorization: Bearer {key}`.

### 1.4 SSE parsing

OpenAI SSE format:
```
data: {"id":"...","choices":[{"delta":{"content":"tok"},"finish_reason":null}]}

data: {"id":"...","choices":[{"delta":{},"finish_reason":"stop"}]}

data: [DONE]
```

New `OpenAiSseBuffer` (does NOT share code with the Anthropic `SseBuffer` in `cloud.rs` — the two SSE schemas are structurally different):
- Buffers bytes, yields complete `data: ` lines.
- Ignores non-`data:` lines (`:` comment heartbeats, `event:`, `id:`, blank lines).
- `data: [DONE]` → emit `TokenChunk { text: "", done: true }`.
- Otherwise parse JSON → `choices[0].delta.content` → `TokenChunk`. `finish_reason != null` → `done: true`.

### 1.5 Error classification

Reuse `InferenceError` variants via a new `classify_openai_compat_status(status, body, model)`:

| HTTP status | `InferenceError` variant |
|-------------|--------------------------|
| 401, 403 | `Auth` |
| 429 | `RateLimited { retry_after }` (parse `Retry-After` header) |
| 404 + body mentions model | `ModelNotFound { model }` |
| 500-599 | `ServiceUnavailable` |
| other | `Other(...)` |

### 1.6 Retry

Pre-stream phase (send request + check status) wrapped in `primer_core::retry::retry_with_backoff`, same pattern as `CloudBackend` and `OllamaBackend`. Once 2xx is received and byte streaming starts, mid-stream errors propagate and the partial turn drops at the dialogue-manager layer.

### 1.7 Network error classification

Reuse the existing `classify_reqwest_error` from `cloud.rs` (already `pub(crate)`, so the new module in the same crate can call it directly — no visibility change needed). Maps `reqwest::Error` connection/timeout variants to `InferenceError::NetworkUnavailable`.

---

## 2. Embedding backend: `primer-embedding/src/openai_compat.rs`

### 2.1 Struct

```rust
pub struct OpenAiCompatEmbedder {
    client: reqwest::Client,
    base_url: String,
    model_name: String,
    api_key: Option<String>,
    dim: usize, // probed at construction
}
```

### 2.2 Construction

`OpenAiCompatEmbedder::new(base_url, model_name, api_key)` — sends a one-shot embed of `"primer"` to discover `dim`, same pattern as `OllamaEmbedder::with_endpoint`. Errors if the probe fails.

### 2.3 `Embedder` impl

`embed(texts)` → `POST {base_url}/v1/embeddings`:

```json
{
  "model": "<model>",
  "input": ["text1", "text2", ...]
}
```

Response:
```json
{
  "data": [
    {"embedding": [0.1, 0.2, ...], "index": 0},
    {"embedding": [0.3, 0.4, ...], "index": 1}
  ]
}
```

Results sorted by `index` to match input order (the spec allows out-of-order responses). Each vector's length validated against `self.dim`.

**Native batching** — unlike `OllamaEmbedder` which loops one-text-per-call, the OpenAI embeddings endpoint accepts the full batch in one request. This is a meaningful throughput improvement for `--reembed` bulk backfills.

### 2.4 Feature gate

New `openai-compat` feature in `primer-embedding/Cargo.toml`:
```toml
openai-compat = ["dep:reqwest", "dep:serde", "dep:serde_json"]
```

Same pattern as the existing `ollama` feature. Pulls only workspace deps already present — no new external crates.

### 2.5 Auth

If `api_key` is `Some`, sends `Authorization: Bearer {key}`.

---

## 3. CLI + engine wiring

### 3.1 `BackendParams` (primer-engine)

Add two fields:
```rust
pub openai_compat_url: String,         // default "http://localhost:8000"
pub openai_compat_api_key: Option<String>,
```

### 3.2 `build_backend` (primer-engine)

New match arm in `build_backend`:
```rust
"openai-compat" => Ok(Arc::new(
    primer_inference::openai_compat::OpenAiCompatBackend::new(
        params.openai_compat_url.clone(),
        model,
        params.openai_compat_api_key.clone(),
    ),
)),
```

### 3.3 Embedder factory (primer-engine)

New `build_openai_compat_embedder(base_url, model, api_key) -> Result<Arc<dyn Embedder>>`, feature-gated on `primer-embedding/openai-compat`. Same dual-stub pattern as `build_fastembed_embedder` / `build_ollama_embedder`.

### 3.4 CLI flags (primer-cli)

| Flag | Default | Env var |
|------|---------|---------|
| `--backend openai-compat` | (no default; explicit choice) | — |
| `--openai-compat-url <url>` | `http://localhost:8000` | `OPENAI_COMPAT_URL` |
| `--openai-compat-api-key <key>` | None | `OPENAI_COMPAT_API_KEY` |
| `--embedder-backend openai-compat` | (no default) | — |
| `--embedder-openai-compat-url <url>` | falls back to `--openai-compat-url` | `OPENAI_COMPAT_URL` |
| `--embedder-openai-compat-model <name>` | (required when embedder backend is openai-compat) | — |

When `--backend openai-compat`, `--model` is required (same as ollama — we don't know what models the server has).

The `--classifier-backend`, `--extractor-backend`, and `--comprehension-backend` flags already accept any backend name and route through `build_backend`. Adding `"openai-compat"` to `build_backend` means these automatically work with `--classifier-backend openai-compat --classifier-model <m>` — no additional wiring needed.

### 3.5 GUI wiring

Deferred. The GUI's `primer-engine` wiring calls the same `build_backend` and `build_*_embedder` factories, so once the engine layer is done the GUI gets OpenAI-compat support with minimal additional work (settings UI + config persistence). Not in scope for this spec.

---

## 4. Dependencies

No new external crates. The inference backend uses `reqwest`, `serde`, `serde_json`, `futures`, `async-trait`, `tracing` — all already workspace deps of `primer-inference`. The embedding backend uses the same set gated behind the `openai-compat` feature, same pattern as the `ollama` feature gate.

---

## 5. Testing

### 5.1 Unit tests (always built)

- **SSE parsing:** `OpenAiSseBuffer` unit tests covering: single-token chunks, multi-token chunks, `[DONE]` termination, `finish_reason` termination, heartbeat/comment lines, malformed JSON (returns parse error, doesn't panic), empty `delta.content` (skip, don't emit empty chunk).
- **Error classification:** `classify_openai_compat_status` tested for each mapped status code.
- **Request serialization:** verify system prompt → system message, role mapping, parameter mapping.

### 5.2 Integration tests (manual, against a running server)

- `#[ignore]` test that hits a real OpenAI-compat server (oMLX or LM Studio) with a simple prompt, asserts streaming response is non-empty.
- `#[ignore]` embedding test that embeds a batch, asserts correct dim and batch size.

### 5.3 Embedder dim-mismatch

Same pattern as `OllamaEmbedder`: if a subsequent `embed` call returns a vector with length != `self.dim`, hard error.

---

## 6. CLAUDE.md updates

Add documentation for the new backend:
- CLI flag reference (existing `--backend` line gains `openai-compat`)
- `BackendParams` gains two fields
- New `build_openai_compat_embedder` factory
- OpenAI SSE format note (vs Anthropic SSE and Ollama NDJSON)
- Feature gate note for `primer-embedding/openai-compat`

---

## 7. Effort estimate

| Component | Estimate |
|-----------|----------|
| `primer-inference/src/openai_compat.rs` | ~200 lines |
| `primer-embedding/src/openai_compat.rs` | ~120 lines |
| `primer-engine/src/wiring.rs` changes | ~40 lines |
| `primer-cli` flag additions | ~30 lines |
| `primer-embedding/Cargo.toml` feature | ~3 lines |
| `primer-inference/src/lib.rs` + `primer-embedding/src/lib.rs` re-exports | ~6 lines |
| Unit tests | ~150 lines |
| CLAUDE.md updates | ~20 lines |
| **Total** | **~570 lines** |
