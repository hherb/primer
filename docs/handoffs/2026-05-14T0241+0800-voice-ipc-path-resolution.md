# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-14T0241+0800 (after opening PR #99 — voice IPC path re-resolution hardening; commit `d6244fe` on `harden/voice-ipc-path-resolution-90`; closes #90).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **767 Rust tests** under default features (up from the previous 739; the +28 is from the `6f8c187` test-additions commit folded into PR #97's squash-merge). 3 ignored. With `--features primer-gui/speech` an additional 5 unit tests in `voice/assets.rs` run for a total of 89 primer-gui tests on that feature. Add `--features primer-kb-load/fastembed` for the embedding-backed sweeps + the real-BGE-M3 recall tests (downloads BGE-M3 ~570 MB on first run; cached afterwards). Plus **135 Python tests** in `data/ingest/` (unchanged this session — Rust-only work).
3. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first — Horst may have made interim changes. Always verify open-issue claims and `git log origin/main` since this brief's "last updated" timestamp before starting.

## Branch status

`harden/voice-ipc-path-resolution-90` carries 1 commit, pushed, and on **PR #99** (https://github.com/hherb/primer/pull/99). Built off `origin/main` at `eb147a9` (PR #97 squash — retrieval-sweep harness dedupe). Once #99 merges, the branch can be deleted.

## What we shipped this session

**Hardened the `download_voice_assets` IPC trust boundary so a compromised webview cannot direct the host to write outside `~/.cache/primer/models/` or fetch from a non-canonical URL.** Pre-change, the frontend echoed the full `MissingAsset` payload (kind + path + suggested_url + size) back to the command; the server used those fields verbatim. Post-change, the frontend echoes only the `kind` strings, and the server re-resolves `path` + `suggested_url` server-side via the existing locale-aware resolver.

**Commits on `harden/voice-ipc-path-resolution-90`:**

- `d6244fe` — `harden(voice): re-resolve asset paths server-side in download_voice_assets (closes #90)`

**Concrete deliverables:**

- **New pure helper `voice::assets::resolve_requested_kinds(home, speech, locale, &[String])`** at [src/crates/primer-gui/src/voice/assets.rs](src/crates/primer-gui/src/voice/assets.rs#L111-L125): calls `resolve_voice_assets` then filters the missing entries by the frontend-supplied `kinds` list. Unknown / already-present kinds drop silently (safe — nothing to download). An `Ok(ResolvedAssets)` from the inner resolver yields an empty Vec so callers can unconditionally iterate.
- **Command signature change** in [src/crates/primer-gui/src/commands/voice.rs](src/crates/primer-gui/src/commands/voice.rs#L334-L356): `download_voice_assets(state, app, kinds: Vec<String>)`. Active locale comes from `state.config.lock().await.learner.locale`, never from the IPC payload.
- **`MissingAsset` is `Serialize`-only** — `Deserialize` deliberately removed so the IPC direction (server → webview) is structurally enforced, not just by inspection.
- **Frontend update** in [src/crates/primer-gui/ui/voice.js](src/crates/primer-gui/ui/voice.js#L151-L156): `invoke("download_voice_assets", { kinds: entries.map(e => e.kind) })`.
- **CLAUDE.md updated** with a gotcha entry documenting the trust-boundary invariant (search "download_voice_assets IPC takes only").
- **5 new unit tests** in `voice/assets.rs` (speech-feature-gated): drops unknown kinds, returns all three on fresh home, returns empty when all present, every resolved path lives under `cache_root(home)`, empty input → empty output.

**Verification:**

- `~/.cargo/bin/cargo test --workspace --no-fail-fast` → 767 passed / 0 failed / 3 ignored (default features)
- `~/.cargo/bin/cargo test -p primer-gui --features speech` → 89 passed / 0 failed / 0 ignored (incl. the 5 new helper tests)
- `~/.cargo/bin/cargo fmt --all -- --check` clean
- `RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets` clean (default features)
- `RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-gui/speech` clean (speech feature)

**Net diff:** 4 files changed, 165 insertions(+), 16 deletions(-).

**Design choices that may be relevant later:**

- **`kinds: Vec<String>` rather than `()` as the payload.** A no-payload `download_voice_assets()` would have been even tighter (zero IPC input), but a kinds list documents user intent and gracefully handles the edge case where the active locale changes between the consent modal appearing and the user clicking Download — the helper intersects requested kinds with what the resolver *currently* says is missing, so a stale-payload race silently no-ops rather than over-downloading. The trust boundary is unchanged either way because kinds are filter input, not write targets.
- **Unknown kinds drop silently rather than erroring.** Hostile kinds like `"executable_payload"` or `"../../../etc/passwd"` simply have no resolver match, so the iteration produces no work. Erroring would have leaked information about which kinds the resolver knows about; silent no-op is the simpler safe behaviour.
- **`Deserialize` removed from `MissingAsset` even though it's a one-way IPC type.** The struct only crosses server → webview; removing `Deserialize` is a structural invariant that catches a future regression at compile time. If someone needs to round-trip it later, the right answer is to introduce a separate echoed-identity type (e.g. `RequestedAssetKind { kind: String }`), not to re-add `Deserialize`.

## What's next

### The most defensible smaller-scope follow-ups

- **Hindi (`hi`) locale pack rollout.** Now that #66 is closed AND the sweep-harness refactor is in place, adding Hindi is a data-only change. The required adds are: `tests/common/hi.rs` (define `QUERIES_HI` mirroring the EN/DE shape), parallel `retrieval_quality_hi.rs` (mirror `retrieval_quality_de.rs`), parallel `retrieval_sweep_hi.rs` + `retrieval_sweep_hybrid_hi.rs` (~50-line shims each via `run_bm25_sweep` / `run_hybrid_sweep`), a `WikiSource` preset in `data/ingest/wiki/source.py`, and a children's-vocabulary corpus source. Wikipedia's "बाल विकिपीडिया" (Bal Vikipedia) is the obvious analogue of Klexikon and Simple English — confirm it's actually live and CC-licensed before commitment. Schema + i18n boundary are already locale-keyed; no Rust core changes expected.
- **#91** — voice: move `VoiceStateCopy` strings into per-locale TOML packs. Currently hardcoded in `commands/voice.rs::VoiceStateCopy::for_locale`. Concrete fix: add a `voice_state` table to `i18n/packs/en.toml` and `de.toml`, expose accessor methods on the existing pack struct, replace the hardcoded match.
- **#92** — voice: `download_voice_assets` needs timeout, resume, and max-size cap. Current implementation has no upper bound on download size and no per-asset timeout; a network hang or hostile-CDN response could lock the consent modal indefinitely. Concrete fix: wrap `reqwest::Client` in a timeout-bearing builder; track bytes_done against `approx_size_mb * 1.2 * 1_048_576` as a soft cap; on partial-file resume, send a `Range:` header if the `.partial` exists.

### Larger queue items (carried forward)

- **Local llama.cpp inference (Phase 1.1).** `LlamaCppBackend` stub at `primer-inference` is the entry point.
- **Voice-loop hardening** (echo cancellation, ambient-noise robustness; Phase 2 is at POC quality, not production). The voice mode (Phase A) GUI work landed in PR #89 — production polish is the still-open piece. Issues #91/92 above are the immediate review-fallout follow-ups.
- **Hardware integration** (Phase 3 — display, audio, enclosure).
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default. The relevant flips: `default = ["embedding"]` in `primer-cli/Cargo.toml` and the `--embedder-backend` CLI default.

### Klexikon corpus follow-ups (carried forward)

- **Klexikon corpus expansion past 66.** Klexikon has ~3000 articles. The 2 corpus gaps from the PR #93 session (gänsehaut reflex; tides on the mond article) would need either expanded articles or additional Klexikon titles to lift. Concrete next-batch acceptance: pick 30-50 more titles in still-thin clusters — body could use Atmung-related topics; life could use Ökosystem, Affe/Pferd/Fledermaus; how-things-work could use Computer/Internet/Telefon/Maschine. Re-run pipeline. No code change required. Once corpus grows past ~150 passages, re-run the sweep harnesses to verify production defaults still flatline at 100% strict from `top_k=5` onward on non-paraphrase queries (and that the BM25 floor's defensive 0.5 hasn't been challenged by the larger statistic).
- **Klexikon license claim spot-check.** Concrete acceptance: WebFetch a sample of ~5 article footers (e.g. `Sonne`, `Atome und Moleküle`, `Skelett`, `Bienen`, `Mondfinsternis`) and verify each footer shows CC-BY-SA-4.0. If a per-page divergence appears, document a per-passage license override field in `WikiSource`. Low-priority.

### Smaller-scope follow-ups still open

- **#86** — primer-gui: avoid double session-DB open on resume (enhancement).
- **#87** — primer-gui: end-to-end resume_session test for cross-locale inheritance (enhancement).
- **#80** — GUI: expose Locale::ALL via a Tauri command instead of hand-mirroring it in settings.js (enhancement).
- **#81** — GUI: settings modal needs a focus trap (enhancement).
- **#71** — GUI: tighten CSP before ship (remove `'unsafe-inline'`).
- **#69** — primer-engine: embedder helpers should return Result, not `std::process::exit`.
- **#46** — explore post-RRF `min_score` as a fifth grid axis in the hybrid sweep. (Now lives in the shared helper at `tests/common/sweep.rs`; the change is a one-axis addition to `HybridSweepConfig` + the loop body.)
- **#40** — aggregate per-source attribution row for Wikipedia (and Klexikon).
- **#41** — tighten disambiguation regex if false positives appear.
- **#22** — cache prepared statements for the corpus-bootstrap path (Phase 0.2; enhancement).
- **#21** — separate `--languages` preference list from bound `--language` locale.
- **#20** — i18n placeholder validator can false-fail on translator narrative text.
- **Failed-batch persistence sidecar (issue #38 optional).** Was deferred during brainstorming; file as new issue if scheduled ingest ever ships.
- **Network-error retry.** `requests.exceptions.ConnectionError` / `Timeout` still propagate unchanged.
- **Pre-commit fmt hook (workflow-level).** Carried forward from PR #94. Drift accumulated 13 files across 5 PRs before CI noticed; a local `cargo fmt --check` pre-commit hook would prevent it. Defer until the next time drift happens.

## Open decisions / risks

Carried-forward open items (still relevant from prior sessions):

- Smoke #6 (Ollama daemon down) deferred verification.
- `is_request()` heuristic conflates config errors with network errors.
- HTTP-date form `Retry-After` silently dropped (true on both Rust and Python sides).
- Pre-flight auth check at startup deferred.
- Voice-mode bridging hook is data-only.
- `close_session` may drop the final classifier task's `turn_classifications` row on a short conversation.
- Identifier-format divergence across structured-output crates (`llm:{model}` vs `llm:{backend}:{model}`).
- `apply_comprehension` insertion policy (only updates concepts already in `learner.concepts`).
- Concepts are monotonic on `learner_concepts` — saved-once stays saved.
- No cancellation token on streaming-task spawns.
- Background-task spawn order is load-bearing on serialized backends.
- "I just don't want to see this word" escape valve for vocab not yet implemented.

**New observations from this session:**

- **The IPC hardening covers the *path* and *URL* attack surface, but not download size or timeout.** A hostile webview can still submit `kinds: ["whisper_model", "whisper_model", "whisper_model"]` (duplicates) — currently the helper dedups by virtue of `resolve_voice_assets` returning each missing kind once, so duplicate submissions are no-ops. But a CDN-side response that serves a 50 GB payload masquerading as the Whisper bin could still fill the user's disk. Issue #92 is the natural follow-up: timeout + size cap.
- **Manual smoke not run yet.** The PR description checklist marks this as `[ ]` deliberately — the changeset is small and well-tested, but voice mode needs a clean cache + GUI launch to confirm the download path is byte-identical from the user's perspective. Run the GUI under `~/.cargo/bin/cargo run --bin primer-gui --features speech` after PR #99 merges and re-verify.
- **`MissingAsset` no longer being `Deserialize` is a one-way invariant.** If a future PR re-adds `Deserialize` to round-trip the type through a different IPC, that PR must NOT also revert the `download_voice_assets` signature back to `missing: Vec<MissingAsset>` — the security guarantee depends on the command signature, not the trait derive. The CLAUDE.md gotcha entry is the durable signpost.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing, with one new pattern added this session.)

- **NEW: Server-side re-resolution at IPC trust boundaries.** Pattern shape: when the webview echoes back a payload that includes both *identity* (kind, id, slug) and *resource locators* (path, URL, fd), have the command take only the identity strings and re-resolve the locators server-side via the same resolver that produced them originally. Pair with a `Serialize`-only output type (no `Deserialize`) to make the trust direction structural. Applies wherever IPC has any chance of being compromised — Tauri webviews, web-socket bridges, IPC to renderer processes.
- **Shared test harness with `*Config` carrier struct + locale-specific shim.** Pattern shape: one helper module (`tests/common/sweep.rs`), one config carrier struct per algorithmic shape (`Bm25SweepConfig`, `HybridSweepConfig`), one entrypoint per shape (`run_bm25_sweep`, `run_hybrid_sweep`), thin per-locale shims that fill the config. Output is the public contract — verify with byte-identical diff against pre-refactor baselines, not arbitrary assertions. The same pattern can be applied to the `retrieval_quality{,_de}{,_hybrid,_hybrid_de}` regression tests if they grow a third locale.
- **Pure functions in dedicated modules** for algorithmic cores — tested directly without I/O.
- **`#[cfg(test)]` test seams with `pub` accessor methods** — zero production cost.
- **Locale-keyed templates in TOML packs** with `{placeholder}` substitution at trait method level.
- **Carrier structs with `disabled()` no-op constructors** for parameter bundles that not all callers need to configure.
- **Lookup-table seeding for new closed-enum variants** — no schema migration needed.
- **Constants in `consts.rs` submodules** (Rust) or top-of-module `_DEFAULT_*` constants (Python). No magic numbers anywhere.
- **TDD discipline.** Tests first; watch them fail; implement to green. **Adapted this session** — for a security-hardening refactor where the new helper has a clear pure-function shape, write tests + implementation in the same edit pass but make sure tests cover the trust-boundary invariant (cache-root containment, hostile-kind drop).
- **File-size hygiene.** Keep modules under 500 lines.
- **Network-injection test seam** for any data-ingest pipeline (`http_client` parameter, substitute `FakeHttpClient`/`KlexikonFakeHttpClient` in tests).
- **Subagent-driven development workflow** for plan execution (or inline executing-plans for small TDD-shaped plans).
- **Defensive sanity tests at the data layer.**
- **Always pin `Default` impls of public structs to `consts::*`, with a drift-prevention test.** Mirrored on the Python side as `RetrySettings.default()` ↔ module-level `DEFAULT_*` consts.
- **Pure helpers + their own unit tests, even in `#[ignore]`'d integration tests.**
- **Frozen dataclasses for process-wide configuration** (the `WikiSource` pattern; `RetrySettings` follows the same shape).
- **String discriminators for strategy selection, with allow-list validation.**
- **Re-run the live ingest after changing any fetch-path helper.** Tiny expected diff = success, anything bigger = a flag worth investigating.
- **Kwarg-injected side-effect functions for TDD seams in Python.**
- **Structural ingest-time defences beat manual probing habits.**
- **Back-compat re-export shims when bulk-editing test imports would dilute a structural-refactor PR.**
- **Plan-then-execute inline for mechanical refactors with a strong test safety net.**
- **Two-commit refactor: "set up the change" then "remove the old".** (Most session refactors are small enough for one commit; the two-commit form is for larger PRs.)
- **Ship the follow-up issue body with explicit acceptance + per-file checklist when a PR ships a transitional state.**
- **Per-locale dataset modules under `tests/common/<pack>.rs`** for benchmark data.
- **Locale-specific cluster sizing via a named const** (`DE_CLUSTERS = 5`).
- **Required-loose-substrings drawn from the canonical article's lead, not the query phrasing.** EN-side convention from PR #45; re-confirmed standing in PR #93. Makes the loose check measure retrieval, not vocabulary alignment.

## Exact commands needed to resume

```bash
# Resume on main (after PR #99 merges):
cd /Users/hherb/src/primer
git status                       # confirm clean
git checkout main
git pull
git log --oneline -10            # the PR-99 squash-merge commit should be near the top

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 767 passed, 0 failed, 3 ignored (default features).

~/.cargo/bin/cargo test -p primer-gui --features speech
# Expected: 89 passed, 0 failed, 0 ignored.

~/.cargo/bin/cargo fmt --all -- --check
# Expected: clean.

RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets
# Expected: clean exit 0 (mirrors CI).

# Speech-feature clippy (slower; downloads Tauri's macro deps on first run):
RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-gui/speech
# Expected: clean exit 0.
```

To exercise the IPC trust boundary manually:

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo run --bin primer-gui --features speech
# In the GUI: toggle voice mode on, accept the consent dialog if assets
# are missing. The frontend now sends only { kinds: [...] }; the host
# resolves paths and URLs server-side. Assets should land under
# ~/.cache/primer/models/ exactly as before.
```

To re-run the German regression benchmarks (both flavours):

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_de
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_hybrid_de
# Real BGE-M3 recall floor (downloads ~570 MB on first run; cached afterwards):
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test retrieval_quality_hybrid_de
```

To re-run the German sweep diagnostics (via the shared helper at `tests/common/sweep.rs`):

```bash
cd /Users/hherb/src/primer/src

# BM25-only (always built; ~250ms wallclock):
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_sweep_de \
    -- --ignored sweep_retrieval_params_de --nocapture

# Hybrid (downloads ~570 MB BGE-M3 on first run; ~78s wallclock when cached):
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed \
    --test retrieval_sweep_hybrid_de \
    -- --ignored sweep_retrieval_params_hybrid_de --nocapture
```

For the Python ingestion pipeline tests (uv-only — never invoke pip directly):

```bash
cd /Users/hherb/src/primer/data/ingest
# (venv was set up per data/ingest/README.md: `uv venv .venv` +
# `uv pip install --python .venv/bin/python -r requirements.txt`)
.venv/bin/pytest tests/
# Expected: 135 passed.
```

For mypy on the ingest tree:

```bash
cd /Users/hherb/src/primer/data/ingest
mypy --python-executable .venv/bin/python simple_wikipedia.py wiki/ retry.py build_whitelist.py
# Expected: Success: no issues found in 7 source files.
```

For real-LLM smoke testing (Anthropic):

```bash
cd /Users/hherb/src/primer/src
RUST_LOG=debug ~/.cargo/bin/cargo run --bin primer -- \
    --backend cloud --name SmokeTester --age 9 --no-persist --verbose 2>&1 | tee /tmp/smoke.log
```

For real-LLM smoke testing in German (66 Klexikon passages auto-loaded):

```bash
cd /Users/hherb/src/primer/src
RUST_LOG=debug ~/.cargo/bin/cargo run --bin primer -- \
    --backend cloud --name Lukas --age 9 --language de --no-persist --verbose 2>&1 | tee /tmp/smoke_de.log
# Expected: KB auto-loads 66 Klexikon passages on locale=de.
```

For re-running the Klexikon ingest (rare; only when the whitelist changes or articles drift):

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/python simple_wikipedia.py --language de
# Writes ../seed/wiki_passages.de.jsonl. Commit any diff.
# Live HTTP traffic to klexikon.zum.de; ~66 sequential requests with 1s pacing
# (per-page parse strategy = no batching; takes ~66s on a warm network).
# 429/5xx retried 3× with backoff (PR #57).
# Post-resolution duplicate-id collisions raise RuntimeError (PR #59).
```

For re-running the Simple English Wikipedia ingest:

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/python simple_wikipedia.py --language en
# Writes ../seed/wiki_passages.en.jsonl. Commit any diff.
# Live HTTP traffic to simple.wikipedia.org; ~2 batched requests of 20 titles.
# 429/5xx retried 3× with backoff (PR #57).
```

For the speech build path: `~/.cargo/bin/cargo build --workspace --features primer-cli/speech`.

For the embedding feature build path:

```bash
~/.cargo/bin/cargo build --workspace --features primer-cli/embedding
~/.cargo/bin/cargo run --bin primer -- --embedder-backend fastembed ...
# First run downloads BGE-M3 (~570 MB) into the fastembed cache.
```

For running the BM25 sweep diagnostic (English):

```bash
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_sweep \
    -- --ignored sweep_retrieval_params --nocapture 2>&1 | tee /tmp/sweep.txt
```

For running the BM25 floor tripwire:

```bash
~/.cargo/bin/cargo test -p primer-kb-load --test bm25_floor_tripwire \
    -- --ignored bm25_score_floor_tripwire --nocapture
```

For running the hybrid sweep (English):

```bash
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed \
    --test retrieval_sweep_hybrid \
    -- --ignored sweep_retrieval_params_hybrid --nocapture 2>&1 | tee /tmp/sweep_hybrid.txt
```

For running the EN hybrid regression test:

```bash
# Structural (always built):
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_hybrid

# Real-recall (--features fastembed, real BGE-M3):
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed \
    --test retrieval_quality_hybrid -- --nocapture
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- If you discover that Horst did interim work that changes the plan, flag it. Always verify open-issue claims and `git log origin/main` since this brief's "last updated" timestamp before starting.
