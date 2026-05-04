# Optimizing LLM Request Order

**Status:** TODO — design doc, not yet implemented.
**Created:** 2026-05-03.
**Trigger context:** code review of PR #14 raised the question of whether the spawn order of the per-turn background tasks (engagement classifier, concept extractor, comprehension classifier) affects end-to-end latency. Short answer: yes, in specific conditions, and the current order is correct only by accident. This doc formalizes the question, sketches the measurement plan, and codifies the design principle.

## Background — the per-turn background tasks

After each `respond_to_streaming` exchange, the dialogue manager spawns two independent `tokio::spawn` tasks (see `crates/primer-pedagogy/src/dialogue_manager.rs`):

1. **Engagement classifier task** — one LLM call, classifies the child's last turn into an `EngagementState`. Persisted to `turn_classifications`. Default timeout: **3 s**.
2. **Post-response chain (extractor → comprehension) task** — one LLM call to extract concepts, then (if any concepts surfaced) a second LLM call to assess comprehension depth per concept. Persisted to `turn_concepts` and `turn_comprehensions`. Default combined timeout: **5 + 5 = 10 s**.

Both tasks must finish (or time out) before the next turn's intent decision via `await_pending_background`, which awaits them in parallel via `tokio::join!`.

A timeout fires from spawn time, not from start time. **A task that spends most of its budget queued behind another never gets to run.**

## The three regimes

Whether spawn order matters depends on what the backend does with concurrent requests:

| Regime | Backend example | Spawn order matters? |
|--------|-----------------|----------------------|
| **Fully parallel** | Anthropic API (HTTP/2 multiplexing); multi-instance Ollama with distinct models per task | No — both tasks run concurrently regardless of spawn order. |
| **Batched-parallel** | Single Ollama instance with `OLLAMA_NUM_PARALLEL ≥ 2` | No (within batch capacity) — Ollama's batched decode loop runs both as concurrent sequences in shared forward passes. |
| **Serialized** | Single Ollama instance with `OLLAMA_NUM_PARALLEL = 1`, or low VRAM forcing single-model + single-slot | **Yes** — second-spawned task queues; its timeout deadline ticks while it waits. |

### The Ollama parallelization model in detail

Per the Ollama FAQ and community docs, three env vars govern concurrency:

- `OLLAMA_NUM_PARALLEL` — max concurrent requests *per loaded model*, processed as a batched-decode group. Default: 1 or 4 depending on available memory.
- `OLLAMA_MAX_LOADED_MODELS` — max distinct models resident in RAM/VRAM at once. Default: ~3× GPU count, or 3 on CPU.
- `OLLAMA_MAX_QUEUE` — server-wide queue ceiling before HTTP 503. Default: 512.

Practical implication: a beefy machine running Ollama with `NUM_PARALLEL = 4` and several distinct models loaded *will* parallelize the Primer's 4 task types (chat + classifier + extractor + comprehension). A resource-constrained device with a single small model loaded and `NUM_PARALLEL = 1` *will* serialize them. Both setups exist in our target audience.

### Cloud providers

Anthropic API multiplexes over HTTP/2; concurrent requests run in parallel server-side. Ditto OpenAI-compatible providers, in practice. For our purposes: cloud is always "fully parallel."

## The design principle

**Calibrate spawn order for the worst case (single-model, single-slot serialized backend). Don't pessimize the better cases.**

Calibration that's correct for the serialized case is by construction harmless for the parallel cases — both tasks still run concurrently there, and the order of two `tokio::spawn` calls separated by microseconds is invisible.

What "calibrate for the worst case" means concretely: in the serialized case, the task with the **shortest timeout budget relative to its work** must spawn first, so its deadline doesn't tick away in the queue. With current defaults (classifier 3 s budget for ~2 s of work; chain 10 s budget for ~8 s of work), the classifier has the tighter relative margin and should spawn first.

## Worked example: why the order matters in the serialized case

Hypothetical Ollama setup, `NUM_PARALLEL = 1`, classifier work ≈ 2 s, chain work ≈ 8 s (unknown order of magnitude — this is what Phase 2 measures).

**Spawn classifier first:**
- t=0: classifier starts, chain queues.
- t=2: classifier completes (within 3 s budget ✓). Chain dequeues, starts.
- t=10: chain completes (within 10 s budget — at deadline, marginal but ✓).

**Spawn chain first:**
- t=0: chain starts, classifier queues.
- t=3: classifier *timeout fires while still in queue* — its deadline ticked from spawn time. Classification row never persists. ✗
- t=8: chain completes.

The current code in `respond_to_streaming` happens to spawn classifier first (line ~605, before the post-response chain at line ~671), but **without a comment explaining the order is load-bearing**. The next refactor that flips the order would silently break Ollama users on resource-constrained devices.

## Implementation plan

### Phase 1 — instrument (DONE — shipped on `feature/comprehension-classifier`)

Status: implemented. Added `tracing::info!(target: "primer::latency", ...)` events at the two spawn sites in `crates/primer-pedagogy/src/dialogue_manager.rs`. No behavior change, no schema/trait/settings changes — pure observability.

**Schema of emitted events:**

- `task = "classifier"` event, fired when the engagement-classifier task completes:
  - `identifier` (string) — the classifier identifier (e.g. `"llm:cloud-anthropic:claude-sonnet-4-6"` or `"stub"`)
  - `queued_ms` (u64) — `start - pre_spawn`; non-zero on serialized backends
  - `work_ms` (u64) — `end - start`
  - `succeeded` (bool) — false on backend error or panic

- `task = "chain"` event, fired when the post-response chain task completes (success OR extractor-error early return):
  - `extractor_id`, `comprehension_id` (strings)
  - `queued_ms` (u64)
  - `extract_ms` (u64) — wall time of the extractor LLM call only
  - `comprehension_ms` (u64) — wall time of the comprehension LLM call only; `0` when candidates were empty (classifier was never invoked, NOT a "0 ms call")
  - `work_ms` (u64) — total chain wallclock including persistence
  - `outcome_label` (string) — `"ok"` or `"extractor_error"`

The per-call sub-timings (`extract_ms`, `comprehension_ms`) go beyond the Phase 1 spec but are essentially free at the same instrumentation point and inform Phase 4's routing-decision question (which task type takes longest on each backend).

**How to read the data:**

```bash
RUST_LOG=primer::latency=info ~/.cargo/bin/cargo run --bin primer -- \
    --backend cloud --name SmokeTester --age 9 2>&1 | grep primer::latency
```

A non-zero `queued_ms` on the chain event (typically the second-spawned task) is the empirical signature of a serialized backend; a near-zero `queued_ms` confirms the backend parallelized. `work_ms_classifier` vs `work_ms_chain` informs the spawn-order calibration in Phase 3.

#### First observed data — Anthropic `claude-sonnet-4-6` (2026-05-04, 3-turn smoke)

Captured during the wrap-up of PR #14 against the production Anthropic API, single live conversation. Three turns shown — sample size too small for tuning decisions, but useful as a starting prior for Phase 2.

| Turn | classifier `work_ms` | chain `extract_ms` | chain `comprehension_ms` | chain `work_ms` |
|------|---------------------:|-------------------:|-------------------------:|----------------:|
| 1    |                 1973 |               1668 |                     2266 |            3936 |
| 2    |                 1712 |               1889 |                     3140 |            5032 |
| 3    |                 2578 |               1980 |                     3622 |            5604 |

`queued_ms = 0` on every event for both tasks — Anthropic parallelizes server-side as expected. Spawn order is irrelevant on cloud; the reorder-for-deadline argument applies only to serialized backends (Ollama with `OLLAMA_NUM_PARALLEL = 1`).

**Three observations worth carrying into Phase 2:**

1. **Comprehension is consistently slower than extraction** (~30-80 % per call: 2266 / 3140 / 3622 ms vs 1668 / 1889 / 1980 ms). Reasonable given comprehension's per-concept structured-JSON output. This was an open question from the PR review and now has a tentative answer.

2. **Chain latency climbed turn-over-turn** (3936 → 5032 → 5604 ms; +42 % over three turns). Could be growing context, increasing concept counts, sonnet variance, or topic complexity. Phase 2 should run 10+ turns of one conversation per backend to see whether the climb continues or plateaus — a monotonic climb means we'll hit the 10 s chain budget around turn 8-10 of a deep conversation, which would be a real problem.

3. **Classifier headroom on sonnet is thin: 14 %** (2578 ms peak vs 3000 ms budget). One slow API call from rate limiting or upstream latency and the classification row drops. Two avenues Phase 2 should evaluate:
   - Bump default classifier timeout for sonnet (5000 ms?), trading wallclock for reliability.
   - Route classifier to `claude-haiku-4-5` while keeping chat on sonnet — likely 3-5× faster classifier calls and cheaper, at the cost of a slightly less nuanced engagement read. The CLI already supports `--classifier-model claude-haiku-4-5 --classifier-backend cloud`; needs the smoke test.

**Persistence cost is negligible**: `chain.work_ms ≈ extract_ms + comprehension_ms` to within 1-3 ms on every event. All wallclock is in the two LLM calls. Optimisations to SQLite paths, candidate-set dedup, etc. would buy nothing measurable. Confirms our intuition that the LLM is the only thing worth optimising here.

### Phase 2 — calibration runs (no code, just data)

Drive a fixed transcript (~10–15 turns covering: bare acknowledgement, fact recall, own-words explanation, application question, topic jump) across this matrix:

| Backend | Model | Setup notes |
|---------|-------|-------------|
| Anthropic | `claude-sonnet-4-6` | parallelized server-side baseline |
| Anthropic | `claude-haiku-4-5` | smaller cloud baseline |
| Ollama | `gemma3:4b`, `NUM_PARALLEL=1` | resource-constrained single-slot worst case |
| Ollama | `gemma3:4b`, `NUM_PARALLEL=4` | same model, batched-parallel mid case |
| Ollama | distinct models per task type, `MAX_LOADED_MODELS≥4` | multi-model best case |
| Mixed CLI | `--model sonnet`, `--classifier-model haiku`, `--extractor-model haiku`, `--comprehension-model haiku` | realistic split (cloud chat, cheap aux) |

Collect `queued_ms` and `work_ms` per task per turn. Write the raw output to a JSONL file under `docs/TODO/calibration-data/<date>.jsonl` so future tuning rounds can compare.

Questions to answer from the data:

1. Does `queued_ms > 0` consistently appear for the second-spawned task in the `NUM_PARALLEL=1` Ollama row? (If yes → serialization confirmed → spawn order is load-bearing.)
2. Is `work_ms_classifier` consistently shorter than `work_ms_chain`? (If yes → current "classifier first" order is right for the serialized case.)
3. Is the ranking stable across models, or does some model invert it? (If unstable → may need configurability per model.)
4. How close are real `work_ms` values to the configured timeouts? (If routinely within 80 % of the budget → bump defaults; if always under 30 % → tighten.)

### Phase 3 — codify the order

Based on Phase 2 findings, almost certainly do all three of:

1. **Add a paragraph-length comment** at the spawn site in `respond_to_streaming` explaining why classifier is spawned before the chain (load-bearing for serialized backends; calibrated against models X / Y / Z; data file at `docs/TODO/calibration-data/<date>.jsonl`).
2. **Lock the order in a test.** Add a unit test that spawns both with a fake serialized backend and asserts the classifier is admitted to the backend first. Cheap to write; prevents accidental reordering in future refactors.
3. **If a model surprises us** (e.g., classifier work consistently exceeds the 3 s default), bump that model's recommended timeout in CLAUDE.md or in a per-model defaults table.

If Phase 2 shows the gap is dramatic and varies wildly per model, *consider* (don't preemptively build) making spawn order configurable via a `BackgroundTaskOrdering` enum in pedagogy settings. Only build configurability if real data shows real variance — not because the abstraction is theoretically nicer.

### Phase 4 — Ollama setup advisory + documentation

Two parts: a runtime advisory for users who unintentionally serialize, and documentation for users who want to deliberately tune.

#### 4a. Startup advisory

When `--backend ollama` (or any `--*-backend ollama`), at startup:

1. Probe the Ollama server's effective config. Ollama exposes `OLLAMA_NUM_PARALLEL` etc. via env on the server side; the HTTP API may or may not surface them. If the API doesn't, we can document the var without auto-detecting.
2. Compute the set of (backend, model) pairs the run will actually hit: chat, classifier, extractor, comprehension. Resolve each via the existing CLI dispatch matrix.
3. If two or more pairs resolve to the *same Ollama (host, model)*, log a one-line `tracing::info!`:
   ```
   Note: chat + classifier + extractor + comprehension all route to ollama:gemma3:4b.
   These tasks will share a single model's parallelism slots. For best latency,
   either: set OLLAMA_NUM_PARALLEL=4 on the server, or use distinct models per
   task (--classifier-model, --extractor-model, --comprehension-model) with
   OLLAMA_MAX_LOADED_MODELS≥4. See docs/TODO/OPTIMIZING_LLM_REQUEST_ORDER.md.
   ```
   Visible only in `--verbose` or `RUST_LOG=info`. Cheap, single-line, opt-in via existing logging — does not nag users who already know what they're doing.

#### 4b. Documentation

Add a "Recommended Ollama configurations" section to CLAUDE.md or a new `docs/OLLAMA_TUNING.md` covering:

- **Resource-constrained device (single small model, low VRAM)**: load one small model (e.g. `gemma3:4b`), set `OLLAMA_NUM_PARALLEL=4`, route all four task types to that model. Effective parallelism comes from Ollama's batched decode. Worst-case fallback if `NUM_PARALLEL=1` is forced: spawn-order calibration (Phase 3) keeps the system functional.
- **Mid-tier device (single GPU, ~16–24 GB VRAM)**: load 2–3 distinct models (e.g. small for classifier/extractor/comprehension, larger for chat), set `OLLAMA_MAX_LOADED_MODELS=3`, `OLLAMA_NUM_PARALLEL=2` per model. CLI: `--model <chat-model> --classifier-model <small-model> --extractor-model <small-model> --comprehension-model <small-model>`.
- **High-tier device or cloud**: distinct model per task type, fully parallelized. CLI: `--model … --classifier-model … --extractor-model … --comprehension-model …` with all four resolving to distinct backends.
- **Cloud (Anthropic)**: no tuning needed; auto-parallel via HTTP/2.

Also document the env var defaults the user might want to override: `OLLAMA_NUM_PARALLEL`, `OLLAMA_MAX_LOADED_MODELS`, `OLLAMA_MAX_QUEUE`. Memory implication: per-model VRAM scales roughly with `parallelism × context_length`, so increasing `NUM_PARALLEL` is not free.

### Phase 5 (lowest priority — only if data warrants) — adaptive ordering

If Phase 2 data shows real variance across models (e.g. classifier work is shorter than chain on small models but longer on larger reasoning models), and Phase 3's static order proves wrong for a meaningful subset of users, then:

- Add a `BackgroundTaskOrdering` config field (enum `ClassifierFirst | ChainFirst | Auto`).
- `Auto` mode keeps a per-model EMA of recent `work_ms` and spawns the task with shorter work first (so the longer one doesn't block the shorter's deadline in serialized backends).

Defer until Phase 2 data justifies the complexity. For most users, a sensible static default + setup advisory + documentation will cover the realistic spread.

## Effort estimate

| Phase | Effort | Code? | Dependency | Status |
|-------|--------|-------|------------|--------|
| 1 — instrument | ~30 min, ~50 lines | yes | none | **DONE** (shipped on `feature/comprehension-classifier`) |
| 2 — calibrate | ~1–2 h running scripts | no (just runs) | Phase 1 + access to Anthropic key + working Ollama | TODO |
| 3 — codify order | 5–15 min | yes (comments + test) | Phase 2 data | TODO |
| 4a — runtime advisory | ~30 min | yes (CLI startup check) | Phase 2 outcome | TODO |
| 4b — documentation | ~30 min | no (docs only) | Phase 2 outcome | TODO |
| 5 — adaptive ordering | unknown — defer | yes | strong evidence from Phase 2 + ongoing data | DEFERRED |

**Recommended sequencing:** Phase 1 has shipped. Do 2 + 3 + 4 in one focused session when an Anthropic key and a working Ollama are both at hand. Defer Phase 5.

## Cross-references

- **Spawn sites:** `crates/primer-pedagogy/src/dialogue_manager.rs` — search for `tokio::spawn` inside `respond_to_streaming`. The classifier spawn is around line 605, the post-response chain spawn around line 671 (line numbers as of commit `b0c34cd` on `feature/comprehension-classifier`).
- **Await coordinator:** `await_pending_background` in the same file (added in PR #14).
- **Settings:** `ClassifierSettings::blocking_timeout`, `ExtractorSettings::blocking_timeout`, `ComprehensionSettings::blocking_timeout`. CLI overrides: `--classifier-timeout-ms`, `--extractor-timeout-ms`, `--comprehension-timeout-ms`.
- **CLI dispatch:** `build_classifier`, `build_extractor`, `build_comprehension` in `crates/primer-cli/src/main.rs` — these decide whether each task shares the chat backend or gets its own instance.
- **Related TODOs (carried in NEXT_SESSION.md):** the `close_session` final-classifier-row drop (open decision #1) is plausibly a different bug, but if Phase 1's instrumentation surfaces a `queued_ms` pattern at session close, the two investigations may converge.
