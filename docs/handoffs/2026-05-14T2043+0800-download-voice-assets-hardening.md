# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-14T2043+0800 (after opening PR #104 — voice-asset download hardening: timeout, resume, and size cap. Closes #92. Branch `harden/download-voice-assets-92`; one commit `a6be9e0` on top of `d1b1af3` + the already-merged PR #101.).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **777 Rust tests** under default features (up from 775 once PR #104 merges; the +2 is the `SpeechSettings.download_timeout_secs` default-from-consts test and the older-config forward-compatibility test). 3 ignored. With `--features primer-gui/speech` an additional 19 tests in `voice/download.rs` run (10 pure-helper unit tests + 7 HTTP integration tests against the in-process `tokio::net::TcpListener` one-shot server + 2 of the new tests are the speech-feature progress event extensions) for a total of 114 primer-gui tests on that feature (up from 95). Add `--features primer-kb-load/fastembed` for the embedding-backed sweeps + the real-BGE-M3 recall tests (downloads BGE-M3 ~570 MB on first run; cached afterwards). Plus **135 Python tests** in `data/ingest/` (unchanged this session — Rust-only work).
3. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first — Horst may have made interim changes. Always verify open-issue claims and `git log origin/main` since this brief's "last updated" timestamp before starting.

## Branch status

`harden/download-voice-assets-92` carries 1 commit (`a6be9e0`), pushed, and on **PR #104** (https://github.com/hherb/primer/pull/104). Built off `origin/main` at `d1b1af3` (`docs(localisation): add contributor manual for translation/locale work`). Once #104 merges, the branch can be deleted.

## What we shipped this session

**Primary work:** wired the three sharp edges the original PR #89 download path carried (no timeout, no resume, no size cap) — the consent modal no longer spins forever on a stalled connection, a killed download resumes from the byte offset it got to, and a redirected URL serving more than the expected size aborts before filling the disk.

**Concrete deliverables:**

- **Three new constants in `primer_core::consts::speech`** at [src/crates/primer-core/src/consts.rs](src/crates/primer-core/src/consts.rs#L194-L218): `DEFAULT_DOWNLOAD_TIMEOUT_SECS` (1800 = 30 min), `DOWNLOAD_SIZE_SAFETY_MULTIPLIER_PCT` (150), `BYTES_PER_MIB` (1_048_576), `PERCENT_DIVISOR` (100). The percent / divisor pair lets the `× pct / 100` math read as percentage-of arithmetic, no magic numbers anywhere.
- **`SpeechSettings.download_timeout_secs: u64`** added to [src/crates/primer-gui/src/config.rs](src/crates/primer-gui/src/config.rs#L308). Default reads from the new const via a `default = "default_download_timeout_secs"` serde callback; older on-disk configs without the field forward-load with the default (pinned in `older_config_without_download_timeout_loads_with_default`). `0` means "no timeout" but is not the default — the brief stays explicit.
- **`DownloadProgressEvent.error: Option<String>`** added to [src/crates/primer-gui/src/voice/download.rs](src/crates/primer-gui/src/voice/download.rs#L55-L57) with `#[serde(skip_serializing_if = "Option::is_none")]`. Set only on the final event emitted from a failed download; success / progress events serialise byte-identically to the pre-issue-#92 JSON shape. Frontend consumers can branch on the kind tag (`"timeout" | "oversize" | "http_status" | "network" | "io" | "no_url"`).
- **`download.rs` refactored into pure helpers + an async core.** Five pure functions (`partial_path_for`, `compute_max_bytes`, `range_header_value`, `parse_content_range_total`, plus a stable `DownloadError::kind()` mapping). `stream_to_path(client, url, dest, max_bytes, on_progress) -> Result<(), DownloadError>` is the async core — it takes a borrowed `reqwest::Client` so the caller decides timeout policy, and the integration tests can use a non-timeout client driven by a `tokio::net::TcpListener` one-shot server (no external mock dep). The Tauri wrapper `download_one` is a thin shell on top.
- **`DownloadError` enum** with six variants (`NoUrl`, `Timeout`, `HttpStatus`, `Oversize { received, cap }`, `Network`, `Io`) — `thiserror`-derived `Display` messages flow into the frontend banner; `kind()` returns the stable `&'static str` tag for the structured event.
- **Partial-file policy change.** Only `DownloadError::Oversize` and `HTTP 416 Range Not Satisfiable` clean up the partial. Every other failure (Timeout, Network drop, transient HttpStatus, I/O) preserves the partial so the next click resumes from the byte offset we got to. The previous always-clean behaviour defeated the entire resume feature.
- **CLAUDE.md gotcha** added at [CLAUDE.md:111](CLAUDE.md#L111) documenting the new download contract, the partial-file policy on failure, and the in-process TcpListener test approach.

**Tests added (TDD, all green):**

- **9 pure-helper unit tests** in `voice::download::tests` covering partial path (incl. multi-extension `.onnx.json` preservation), max-bytes math (`× 150 % × MiB`), open-ended range header form, `Content-Range total` parsing (known total + `*` + malformed), `DownloadError::kind()` stable strings, and the new `DownloadProgressEvent` error-field serialisation (both error-set and error-absent).
- **7 HTTP integration tests** using the inline one-shot `tokio::net::TcpListener` server: 200 happy path, 206 resume with Range header echo verification (case-insensitive — reqwest sends lowercase headers), 200 ignoring Range overwrites the stale partial, oversize aborts with cleanup, timeout preserves partial for resume, 404 → `HttpStatus(404)`, 416 → cleanup + `HttpStatus(416)`.
- **2 config tests** — `download_timeout_secs` default reads from consts; older config without the field forward-loads with the default.

**Verification:**

- `~/.cargo/bin/cargo test --workspace` → 777 passed / 0 failed / 3 ignored (default features; +2 from baseline 775)
- `~/.cargo/bin/cargo test -p primer-gui --features speech` → 114 passed / 0 failed (+19 from baseline 95)
- `~/.cargo/bin/cargo fmt --all -- --check` clean
- `RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets` clean (default features)
- `RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-gui/speech` clean

**Net diff:** 5 files changed, 831 insertions(+), 91 deletions(-). The bulk is in `download.rs` (orig 162 lines → 838 lines incl. tests + docs).

**Design choices that may be relevant later:**

- **`tokio::net::TcpListener` over a mock-server dev-dep.** A handful of `TcpListener::bind("127.0.0.1:0")` + `tokio::spawn` lines beats adding `httpmock` / `wiremock` as a dev-dep — the protocol surface I needed (request capture for header inspection + canned 200/206/416/404 responses + a "never reply" stall) is small enough that bespoke fits in a screen of code. Pattern is now in place if other crates ever need the same shape.
- **Borrowed `reqwest::Client` parameter on `stream_to_path`.** Lets the test layer use a non-timeout client (per-test timeouts via the test framework) and the production wrapper bind `.timeout(...)` to a real one. Avoids building a fresh client per request while still keeping the lifetime ownership obvious.
- **Five pure helpers + an async streaming core.** Mirrors the codebase's preference for pure-function modules with their own unit tests. The async function is still testable in isolation (no Tauri AppHandle) because progress is delivered through an `FnMut` callback that the Tauri wrapper substitutes with a closure that captures `AppHandle` and emits events.
- **`#[serde(skip_serializing_if = "Option::is_none")]` on the new `error` field.** Preserves the existing happy-path JSON shape exactly (frontend consumers that destructure `{ asset_id, bytes_done, bytes_total }` keep working unchanged). The kind tag joins the payload only on failure.
- **Stable kind strings.** `DownloadError::kind()` returns `&'static str` — those strings are part of the IPC contract now. The unit test `download_error_kind_strings_are_stable_for_frontend` pins the mapping.
- **Partial-file policy change explained in the doc comment.** Anyone reading `stream_to_path` sees "only Oversize and 416 clean up the partial; everything else preserves it for resume" before they touch the cleanup branch. The reasoning (oversize is tainted, 416 is stale, everything else is the whole point of resume) is right there.

## What's next

### The most defensible smaller-scope follow-ups

- **Frontend kind-aware messaging in the consent modal.** The new `DownloadProgressEvent.error` field is ready for consumption — `ui/voice.js` could branch on it to render kind-specific copy ("Connection stalled. Retry?", "Upstream URL served more than expected; pick a different model?") and offer kind-specific affordances (Retry with longer timeout, Switch to a different voice in Settings → Speech). Concrete acceptance: `voice.js` progress listener observes `evt.payload.error` and replaces the generic "download failed: ..." banner with a kind-specific message when set; a tiny test (manual smoke) confirms the modal renders the right copy for each of the six kinds. Today the existing banner shows the stringified `DownloadError::Display` which is already user-readable; the kind-tag branching is a polish step.
- **Hindi (`hi`) locale pack rollout.** Even more attractive after PRs #101 and #104 — `[voice_state]` is now data-only, the download path is hardened, and the production trust boundaries are in place. Adding Hindi means: `tests/common/hi.rs` (define `QUERIES_HI` mirroring the EN/DE shape), parallel `retrieval_quality_hi.rs` (mirror `retrieval_quality_de.rs`), parallel `retrieval_sweep_hi.rs` + `retrieval_sweep_hybrid_hi.rs` (~50-line shims each via `run_bm25_sweep` / `run_hybrid_sweep`), a `WikiSource` preset in `data/ingest/wiki/source.py`, and a children's-vocabulary corpus source. Wikipedia's "बाल विकिपीडिया" (Bal Vikipedia) is the obvious analogue of Klexikon and Simple English — confirm it's actually live and CC-licensed before commitment. Schema + i18n boundary are already locale-keyed; no Rust core changes expected.

### Larger queue items (carried forward)

- **Local llama.cpp inference (Phase 1.1).** `LlamaCppBackend` stub at `primer-inference` is the entry point.
- **Voice-loop hardening** (echo cancellation, ambient-noise robustness; Phase 2 is at POC quality, not production). The voice mode (Phase A) GUI work landed in PR #89; #91 closed with PR #101; #92 closes with PR #104. The remaining Phase 2 polish is the still-open piece.
- **Hardware integration** (Phase 3 — display, audio, enclosure).
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default. The relevant flips: `default = ["embedding"]` in `primer-cli/Cargo.toml` and the `--embedder-backend` CLI default.

### Klexikon corpus follow-ups (carried forward)

- **Klexikon corpus expansion past 66.** Klexikon has ~3000 articles. The 2 corpus gaps from the PR #93 session (gänsehaut reflex; tides on the mond article) would need either expanded articles or additional Klexikon titles to lift. Concrete next-batch acceptance: pick 30-50 more titles in still-thin clusters. Re-run pipeline. No code change required. Once corpus grows past ~150 passages, re-run the sweep harnesses to verify production defaults still flatline at 100% strict from `top_k=5` onward on non-paraphrase queries.
- **Klexikon license claim spot-check.** Concrete acceptance: WebFetch a sample of ~5 article footers and verify each footer shows CC-BY-SA-4.0. If a per-page divergence appears, document a per-passage license override field in `WikiSource`. Low-priority.

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
- **Network-error retry.** `requests.exceptions.ConnectionError` / `Timeout` still propagate unchanged (Python ingest side).
- **Pre-commit fmt hook (workflow-level).** Carried forward from PR #94.
- **Probe-function duplication between CLI and GUI.** Both `primer-cli/src/main.rs::probe_espeak_ng_data` and `primer-gui/src/lib.rs::probe_espeak_ng_data` carry byte-identical logic except for the log channel. Low-priority refactor.

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

- **Manual smoke not run yet for PR #104.** All seven HTTP integration tests pass against an in-process `tokio::net::TcpListener` server, which mirrors RFC 7233 semantics faithfully — but the real-network smoke (interrupt a download against `huggingface.co`, click Download again, watch the bytes resume from the right offset) hasn't been run. Recommend doing this before merge:
  1. From `src/`: `~/.cargo/bin/cargo run -p primer-gui --features speech`
  2. Toggle voice mode → consent modal → click Download → wait for ~50 MB to land in the `.partial` → kill the binary
  3. Re-launch, click Download → tail the Whisper `.partial` size in `~/.cache/primer/models/whisper/` and confirm it grows from ~50 MB, not from 0
  4. Repeat with the network unplugged mid-download → confirm the `.partial` is preserved
  5. (Optional) Edit `gui-config.json` to set `speech.download_timeout_secs: 5`, start a download, confirm it aborts within 5 s with a timeout banner
- **The `error` field on `DownloadProgressEvent` is data-only ready for the frontend.** No JS code consumes the new field yet — the existing banner shows the stringified `DownloadError::Display` message, which is already user-readable enough to meet the issue acceptance. A future polish PR could add kind-aware modal copy ("Connection stalled. Retry?") and kind-specific affordances (Retry-with-longer-timeout button on Timeout; Pick-a-different-model link on Oversize).
- **Test seam is now in place for the next reqwest-driven download in the codebase.** `stream_to_path(&client, url, dest, max_bytes, on_progress)` + the inline TcpListener server pattern can be copy-pasted into any future crate that needs HTTP-with-resume. Embedder model downloads, for example, currently use `fastembed-rs`'s built-in fetcher; if we ever want timeout/resume parity for those, the lift is a few hundred lines.
- **Partial-file policy is a behaviour change, not just a code refactor.** The previous PR-#89 contract was "always clean partial on graceful error"; the new contract is "only clean on Oversize / 416". Any future code that depends on the old behaviour (e.g. cleanup hooks that don't reach for `.partial` files) needs to either explicitly clean from the call site or trust the new policy. CLAUDE.md gotcha makes this explicit.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing.)

- **REINFORCED: In-process `tokio::net::TcpListener` for HTTP behavior tests.** Pattern shape: `bind 127.0.0.1:0`, `tokio::spawn` an accept loop that reads until `\r\n\r\n`, captures the request, returns a handler-supplied raw response. No dev-dep. Reusable across crates.
- **REINFORCED: Borrowed client / `FnMut` callback test seam for async streaming.** Pattern shape: async core takes `&Client` + `FnMut(...)`, Tauri-style wrapper substitutes the AppHandle-capturing closure. Lets the integration test layer drive the protocol-level behaviour without any framework coupling.
- **Pack-side i18n for any locale-keyed display string the GUI surfaces.** Pattern shape: data struct in `primer-pedagogy::prompt_pack`, single accessor on `PromptPack` returning `&Struct`, `[section]` table in each `prompts/<pack>.toml`, structural validator at load (no-empty / no-placeholder), GUI-side `Serialize`-flavoured equivalent that calls `prompt_pack::load_cached(locale).expect(...)` and clones the strings. Adding a new locale is a TOML-table append.
- **Server-side re-resolution at IPC trust boundaries.** Pattern shape: when the webview echoes back a payload that includes both *identity* (kind, id, slug) and *resource locators* (path, URL, fd), have the command take only the identity strings and re-resolve the locators server-side. Pair with a `Serialize`-only output type to make the trust direction structural.
- **Shared test harness with `*Config` carrier struct + locale-specific shim.** Pattern shape: one helper module (`tests/common/sweep.rs`), one config carrier struct per algorithmic shape, one entrypoint per shape, thin per-locale shims that fill the config. Output is the public contract — verify with byte-identical diff against pre-refactor baselines.
- **Pure functions in dedicated modules** for algorithmic cores — tested directly without I/O.
- **`#[cfg(test)]` test seams with `pub` accessor methods** — zero production cost.
- **Locale-keyed templates in TOML packs** with `{placeholder}` substitution at trait method level.
- **Carrier structs with `disabled()` no-op constructors** for parameter bundles that not all callers need to configure.
- **Lookup-table seeding for new closed-enum variants** — no schema migration needed.
- **Constants in `consts.rs` submodules** (Rust) or top-of-module `_DEFAULT_*` constants (Python). No magic numbers anywhere.
- **TDD discipline.** Tests first; watch them fail; implement to green. **Reinforced this session** — the 7 HTTP integration tests were written before the new partial-file policy was implemented; two of them failed initially (header-case mismatch), the fix was a one-line `.to_lowercase()`, and they stayed green throughout the rest of the refactor.
- **File-size hygiene.** Keep modules under 500 lines where reasonable. `download.rs` is now 838 lines including doc comments and tests; the production body alone is ~280 lines. The integration-test module accounts for the bulk of the additional bytes — the test surface IS the documented contract.
- **Network-injection test seam** for any data-ingest pipeline (`http_client` parameter, substitute `FakeHttpClient` in tests).
- **Subagent-driven development workflow** for plan execution (or inline executing-plans for small TDD-shaped plans).
- **Defensive sanity tests at the data layer.**
- **Always pin `Default` impls of public structs to `consts::*`, with a drift-prevention test.**
- **Pure helpers + their own unit tests, even in `#[ignore]`'d integration tests.**
- **Frozen dataclasses for process-wide configuration** (the `WikiSource` pattern; `RetrySettings` follows the same shape).
- **String discriminators for strategy selection, with allow-list validation.**
- **Re-run the live ingest after changing any fetch-path helper.**
- **Kwarg-injected side-effect functions for TDD seams in Python.**
- **Structural ingest-time defences beat manual probing habits.**
- **Back-compat re-export shims when bulk-editing test imports would dilute a structural-refactor PR.**
- **Plan-then-execute inline for mechanical refactors with a strong test safety net.**
- **Two-commit refactor: "set up the change" then "remove the old".** (Most session refactors are small enough for one commit; the two-commit form is for larger PRs.)
- **Ship the follow-up issue body with explicit acceptance + per-file checklist when a PR ships a transitional state.**
- **Per-locale dataset modules under `tests/common/<pack>.rs`** for benchmark data.
- **Locale-specific cluster sizing via a named const** (`DE_CLUSTERS = 5`).
- **Required-loose-substrings drawn from the canonical article's lead, not the query phrasing.**

## Exact commands needed to resume

```bash
# Resume on main (after PR #104 merges):
cd /Users/hherb/src/primer
git status                       # confirm clean
git checkout main
git pull
git log --oneline -10            # the PR-104 squash-merge commit should be near the top

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 777 passed, 0 failed, 3 ignored (default features).

~/.cargo/bin/cargo test -p primer-gui --features speech
# Expected: 114 passed, 0 failed, 0 ignored.

~/.cargo/bin/cargo fmt --all -- --check
# Expected: clean.

RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets
# Expected: clean exit 0 (mirrors CI).

# Speech-feature clippy (slower; downloads Tauri's macro deps on first run):
RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-gui/speech
# Expected: clean exit 0.
```

To exercise the hardened download path manually (recommended before PR #104 merges):

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo run --bin primer-gui --features speech
# In the GUI: toggle voice mode → consent modal → click Download.
# Mid-download, kill the binary (Cmd-Q or kill -9). Re-launch and click
# Download again. The Whisper .partial in ~/.cache/primer/models/whisper/
# should grow from the previous offset, not from 0.
#
# To test the size cap, edit gui-config.json speech.overrides.en to point at
# a known-larger model; the download should abort with an Oversize error.
#
# To test the timeout, set speech.download_timeout_secs: 5 in gui-config.json
# and start a download; it should abort with a timeout banner.
```

To re-run just the new download test suite (fast):

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-gui --features speech voice::download::
# Expected: 19 passed, 0 failed.
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

For the speech build path: `~/.cargo/bin/cargo build --workspace --features primer-cli/speech`.

For the embedding feature build path:

```bash
~/.cargo/bin/cargo build --workspace --features primer-cli/embedding
~/.cargo/bin/cargo run --bin primer -- --embedder-backend fastembed ...
# First run downloads BGE-M3 (~570 MB) into the fastembed cache.
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- If you discover that Horst did interim work that changes the plan, flag it. Always verify open-issue claims and `git log origin/main` since this brief's "last updated" timestamp before starting.
