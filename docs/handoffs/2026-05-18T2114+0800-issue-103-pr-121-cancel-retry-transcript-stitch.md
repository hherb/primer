# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-18T2114+0800 (PR #121 open on `voice/cancel-retry-transcript-stitch-issue-103` closing #103 — accumulates STT transcript across the voice-loop's cancel-and-retry iterations so the first half of a mid-sentence-cancelled utterance is no longer discarded; commit `9a17aa7`, CI in flight at the time of this brief. PR #120 closing #81 merged earlier the same day as commit `f2fae07`. PR #119 closing #71 merged earlier the same day as commit `3f12c53`. PR #118 closing #112 merged earlier the same day as commit `1d8a1d7`.)

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. **Check PR #121 first** — `gh pr view 121` and `gh pr checks 121`. If it merged while this brief was sitting, switch back to `main`, `git pull`, and proceed to follow-ups. If it's still open and CI is green, consider running the manual voice-mode smoke (see the bottom of this brief) before merging.
3. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green on `main` (post-merge) and on `voice/cancel-retry-transcript-stitch-issue-103` (pre-merge): **856 Rust tests** under default features (unchanged from the post-#120 baseline — the new tests live behind the `voice-loop` cargo feature so they don't appear in the default-features count; the prior brief's "852" was an undercount). 3 ignored. Voice-loop feature: `cargo test -p primer-speech --features voice-loop` is **84 / 0 / 2** (was 82; +2 new tests). With `--features primer-gui/speech`: **146 primer-gui tests** (was 142 in the prior brief; the +4 stems from the prior brief's undercount, not new work this session). With `--features primer-cli/speech`: primer-cli still has **12 tests**; same count under `--features primer-cli/speech,primer-cli/macos-native`.
4. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first. Always verify open-issue claims and `git log origin/main` since this brief's "last updated" timestamp before starting.

## What we shipped this session

### Commit `9a17aa7` (branch `voice/cancel-retry-transcript-stitch-issue-103`, PR #121 open, closes #103) — stitch transcript across cancel-and-retry

When silero trips a mid-sentence `SpeechStart` during `LATENT_THINK`, the LLM call is cancelled and the inner loop re-iterates after the next `SpeechEnd`. Pre-fix, each iteration **reassigned** `transcript_so_far` to just the current `finalize()` output, so the second iteration permanently dropped the first half of the child's utterance — both the bubble emission via `on_transcript_finalized` and the retried LLM call only saw the tail. The issue body laid out three fix options; this PR ships **option 2 (accumulate concatenation)** — the smallest patch that makes the path correct today. Option 1 (a `peek()` method on `StreamingSpeechToText` backed by whisper-cpp-plus `process_step`) remains the principled long-term refactor once the upstream trait exposes partial extraction.

The commit ships:

- **`src/crates/primer-speech/src/voice_loop/state_machine.rs`** — three deltas in the same file:
  - In the `run_loop_inner` LATENT_THINK loop, `let mut transcript_so_far: String;` (declared-but-uninit, then reassigned each iter) becomes `let mut transcript_so_far = String::new();` and each iteration's new (trimmed) STT tokens are appended with a single separating space rather than overwriting. The append is gated on `!new_text.is_empty()` so an iteration that finalizes to nothing doesn't introduce a phantom trailing space; the existing `if transcript_so_far.is_empty() { break; }` check now correctly reflects "nothing accumulated across all iterations" rather than "this iteration was empty."
  - In the `mocks` sub-module, new `ScriptedStreamingStt` mock: takes a `Vec<impl Into<String>>` and drains the queue one entry per `open_session()`. Once exhausted, further sessions finalize to the empty string (which the run-loop's empty-check handles gracefully). The mock makes session-specific finalize text scriptable — the existing `MockStreamingStt` returns the same canned text on every session, so it was structurally unable to test the bug.
  - Two new tests: `scripted_stt_drains_queue_then_returns_empty` is a sanity check pinning the mock's contract; `cancel_and_retry_stitches_full_transcript` is the issue #103 regression — drives the same `SpeechEnd → SpeechStart → SpeechEnd` flow as the existing `cancel_on_resumed_speech_retries_after_continuation` test but with a scripted STT whose two sessions return `"why does"` and `"the sky look blue"`. Asserts (a) `result == vec!["why does the sky look blue"]`, (b) the responder's second `respond(...)` call captured the stitched text, and (c) the observer's `on_transcript_finalized` fired with the stitched text. Before the fix, all three assertions failed at (a) with `left: ["the sky look blue"], right: ["why does the sky look blue"]`.

**Branch:** `voice/cancel-retry-transcript-stitch-issue-103`.
**Tests:** `cargo test --workspace`: **856 / 0 / 3** (unchanged — `voice-loop`-gated tests don't appear in the default-features count). `cargo test -p primer-speech --features voice-loop`: **84 / 0 / 2** (was 82 + 2 new). `cargo test -p primer-cli --features speech`: 12/0/0. `cargo test -p primer-cli --features speech,macos-native`: 12/0/0. `cargo test -p primer-gui --features speech`: 146/0/0. fmt + clippy `-D warnings` clean on default features and on `--features primer-gui/speech` and on `-p primer-speech --features voice-loop`.

**Manual voice-mode smoke not run** — left to the merger. The structural fix is exhaustive (the bug was a per-iteration reassignment vs. append in a single Rust source line, and the regression test pins it), but visual verification by speaking a sentence with an intentional mid-sentence pause would be the cleanest cross-check that production whisper behaves the same way the mock predicts. Commands at the bottom of this brief.

**Why this fix and not a `peek()` refactor:** the existing trait `StreamingSpeechToText` exposes `open_session() -> TranscriptionSession` and the session's `push_audio` / `finalize` methods. Adding `peek()` would (a) require the production whisper-cpp-plus backend to grow a `process_step`-style partial extractor that isn't on the upstream trait surface today, and (b) cascade through every backend, including the mocks. The accumulate-on-loop approach is a single-site change in the consumer, doesn't touch the trait, and reads correctly against the mock (each `open_session` is a fresh session, no overlap to deduplicate) and against production whisper (each `open_session()` constructs a fresh `WhisperContext`, no overlap). If a future STT backend ever buffers across sessions, dedup will need to be added at the append site.

### Earlier in this session day (already merged before this branch was opened)

- **PR #120** (commit `f2fae07`, closes #81): migrate settings + voice-consent modals to native `<dialog>` for a UA-supplied focus trap. Eight new tests in `primer-gui::modal_dialog_contract` pin the contract via `include_str!`.
- **PR #119** (commit `3f12c53`, closes #71): tighten CSP by dropping `'unsafe-inline'` from `script-src` and `style-src`. New 92-line `csp.rs` module pins the policy via three `#[cfg(test)]` tests.
- **PR #118** (commit `1d8a1d7`, closes #112): drop dummy whisper/piper flag requirements on the macOS-native build. Cfg-gates the four CLI flags + their `SpeechLoopConfig` mirrors + `validate_speech_assets` under `not(all(target_os = "macos", feature = "macos-native"))`. Two new tests, each gated to a complementary cfg, pin both speech builds in CI.

## What's next

### Merge PR #121 (priority)

PR #121 is open at https://github.com/hherb/primer/pull/121 with the commit body as the PR description. Acceptance criteria for merging:

- CI green: `cargo test (default features)` status check passes.
- **Manual voice-mode smoke run** (recommended; see the bottom of this brief for the script). The structural fix is exhaustive, but a visual cross-check against production whisper would catch any blind spot the mock didn't anticipate.
- After merge: pull `main`, delete the local + remote branch (`git branch -D voice/cancel-retry-transcript-stitch-issue-103 && git push origin --delete voice/cancel-retry-transcript-stitch-issue-103`), and proceed to the follow-ups below.

### Voice-loop follow-ups (after PR #121 lands)

- **`peek()` refactor on `StreamingSpeechToText`** — the principled long-term fix that the issue body laid out as option 1. Requires the whisper-cpp-plus backend to grow a `process_step`-style partial extractor (the comment at the old reassignment site already gestured at this). Would replace the finalize-and-reopen pattern entirely with a peek+finalize-on-turn-boundary pattern, which is also more aligned with whisper's natural streaming model. Not opening an issue today; the accumulate-on-loop fix is good enough for the current trait surface. Re-open this when the whisper-cpp-plus trait grows partial-extract or when the voice-loop hardening pass for Phase 2 is scoped.
- **STT-overlap deduplication** — if a future STT backend ever buffers across sessions (whisper-cpp-plus's `process_step` could in principle re-emit overlap on a peek-then-finalize), dedup will need to be added at the append site in `state_machine.rs`. Not a concern today; production whisper backends construct a fresh `WhisperContext` per `open_session()`.

### macOS-native speech follow-ups (open after #112 landed)

- **#114** — speech(macos-native): stream PCM chunks to speaker as `AVSpeechSynthesizer` emits them (cut time-to-first-audio). Larger; touches the synthesis path. The current path buffers the full utterance before pushing to cpal; streaming would let the user hear the start of the response sooner. **This is the only macOS-native speech follow-up still open** after #112 landed.

Acceptance criteria for #114 (sketch — refine before implementing):
- `AVSpeechSynthesizer.writeUtterance:toBufferCallback:` is already chunk-by-chunk; the change is plumbing the callback's `AVAudioPCMBuffer` into the speaker ringbuf directly instead of accumulating into a single `Vec<f32>` and pushing post-synthesis.
- Time-to-first-audio (TTFA) measured from end of LLM streaming to first speaker sample should drop substantially. Pick a smoke phrase and pin a TTFA budget in a manual smoke (no test; the smoke binary `examples/tts_macos_pcm_smoke.rs` is the right home for an instrumented variant).
- The PCM-callback chunk-size assumption that `examples/tts_macos_pcm_smoke.rs` validates today must still hold under the streaming path.
- The `is_speaking` echo-suppression invariant (mic muted while speaker is producing audio) must not regress — extend the drain-hook logic so it waits for the *last* chunk to drain, not the first.

### Focus-trap follow-ups (carried forward — #81 landed)

- **Tab-ring smoke for every focusable in settings.** With the browser-supplied focus trap, the cycle goes through every tabbable descendant. The settings modal has many — backend select, model input, ollama URL, API-key radios + input, three subsystem groups, embedder + model + URL, vocab/breaks numerics, persistence fields, speech mic-silence + auto-download checkbox, four locale override cards with four inputs each. A future a11y pass should confirm every step in the ring is visibly focused (no skipped buttons, no stuck-at-end-of-form behaviour). Not opening an issue today; the structural fix is done.
- **`<dialog>` `inert` semantics on the background.** Native `<dialog>` opened via `showModal()` makes the rest of the document inert — clicks pass through to the backdrop only. Worth confirming visually that the chat shell behind the dim overlay doesn't accept keyboard or pointer input.

### Hindi locale follow-ups (carried forward — not touched this session)

- **Native-speaker review of `prompts/hi.toml`.** Grep `# REVIEW:` for the blocks flagged for review. Critical items: tense register (तुम vs. आप), age-band vocabulary markers (तत्सम / Sanskrit-rooted vocabulary), factual-prefix list (Hindi syntax places question words at the end so prefix-matching is weak — consider setting `factual_prefixes = []` and relying entirely on the LLM-engagement-classifier path), `[voice_state]` UI copy (cramped in Devanagari).
- **Hindi children's-vocabulary corpus.** Three candidate sources documented in `docs/localisation/hi/README.md`:
  - **NCERT textbooks** ([ncert.nic.in](https://ncert.nic.in/)) — Indian government textbooks; "free to use for educational purposes" claim needs spot-checking.
  - **Pratham Books StoryWeaver** ([storyweaver.org.in](https://storyweaver.org.in/)) — CC-BY on most books but varies per book; ingest needs per-book license check.
  - **Wikisource Hindi** ([hi.wikisource.org](https://hi.wikisource.org/)) — children's literature; mostly literary, not encyclopedic.
- **`tests/common/hi.rs`** + retrieval-quality / sweep tests for `hi` once a corpus lands.
- **Real-LLM smoke** against `--backend cloud --language hi` and at least three local Ollama models. Populate `docs/locale/models/HINDI.md`.
- **The flip-to-stable PR** when the above are ready: edit `[meta] status = "stable"` in `hi.toml` + add `Self::Hindi` to `Locale::ALL` + remove `# REVIEW:` markers + drop the preview-banner section from `hi/README.md`. Single commit.

### OpenAI-compat backend follow-ups (carried forward)

- **Real-server smoke testing.** Spin up oMLX (Apple Silicon MLX-native server) and one of {LM Studio, vLLM, llama.cpp `--server`}; run `--backend openai-compat --openai-compat-url http://localhost:8000 --model <model>` against each; confirm SSE streaming, error classification, and embedder round-trip. Particularly check the Apple-Silicon throughput claim (the spec cites 20–40% gains via MLX vs. Ollama on the same hardware).
- **GUI wiring.** The spec scopes GUI wiring as a deferred follow-up; today the OpenAI-compat backend is reachable only via the CLI. A future PR should mirror the existing `--backend ollama` / `--backend cloud` GUI surface (settings modal + backend dispatch in `primer-engine`'s GUI consumer) for the new backend.
- **Model evaluation page.** A `docs/openai-compat-models.md` or extension to existing per-locale model pages could track which models behave well behind which servers.

### Carried-forward larger items

- **Branch-protection-on-main remains the structural fix** that PR #109 set up the local-hook layer for. To close the gap at the merge boundary, the repo owner needs to flip a GitHub setting: Settings → Branches → Add rule for `main` → require status check `cargo test (default features)` → require branches up to date before merge. One-time UI click; not a code change.
- **Local llama.cpp inference (Phase 1.1).** `LlamaCppBackend` stub at `primer-inference` is still the entry point. The OpenAI-compat path partially obviates this since llama.cpp's `--server` is already reachable via the new backend, but a direct llama.cpp embedding (without the HTTP hop) remains the long-term Phase 1 goal.
- **Voice-loop hardening** (echo cancellation, ambient-noise robustness; Phase 2 is at POC quality). Voice mode (Phase A) GUI work landed in PR #89; #91 closed with PR #101; #92 closed with PR #104; #102 closed with PR #110; #112 closed with PR #118. PR #121 closing #103 is one transcript-stitching bug fix in this area; #114 expands the macOS-native polish.
- **Hardware integration** (Phase 3 — display, audio, enclosure).
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default.

### Klexikon corpus follow-ups (carried forward)

- **Klexikon corpus expansion past 66.** Klexikon has ~3000 articles. The 2 corpus gaps from PR #93 (`gänsehaut` reflex; tides on the `mond` article) would need either expanded articles or additional Klexikon titles. Concrete next-batch acceptance: pick 30-50 more titles in still-thin clusters. Re-run pipeline. No code change required. Once corpus grows past ~150 passages, re-run the sweep harnesses to verify production defaults still flatline at 100% strict from `top_k=5` onward on non-paraphrase queries.
- **Klexikon license claim spot-check.** WebFetch a sample of ~5 article footers and verify each shows CC-BY-SA-4.0. Low-priority.

### Smaller-scope follow-ups still open

Verified against `gh issue list` 2026-05-18T2114+0800 (no new issues opened since the prior brief; #103 will close on PR #121 merge; #81 closed by PR #120; #71 closed by PR #119; #112 closed by PR #118):

- **#114** — voice(macos-native): stream PCM chunks to speaker as AVSpeechSynthesizer emits them.
- **#98** — refactor(tests): split `tests/common/sweep.rs` into `bm25`/`hybrid` submodules (enhancement). **Defer until Hindi or another third locale lands** — issue body explicitly recommends this.
- **#46** — Hybrid sweep: explore post-RRF `min_score` as a fifth grid axis.
- **#41** — data/ingest: consider scoping disambiguation regex to lead-sentence patterns. **Body explicitly defers until a real article gets falsely rejected.**
- **#40** — data/ingest: aggregate per-source attribution for the Wikipedia layer. **Body notes this is not blocking — option 3 (UI-side aggregation) works today.**
- **#22** — primer-knowledge: cache prepared statements for the corpus-bootstrap path (Phase 0.2; enhancement). **Body explicitly defers as premature optimisation — current corpus is 91 + 66 passages, not millions.**
- **#21** — CLI: separate `--languages` preference list from bound `--language` locale. **Body notes nothing in the engine reads `languages` today.**
- **#20** — i18n: placeholder validator can false-fail on translator narrative text. **Body explicitly defers until a real translator hits it.**

### Out-of-issue-tracker follow-ups still standing

- **Failed-batch persistence sidecar (issue #38 optional follow-up).**
- **Network-error retry on Python ingest side.**
- **Probe-function duplication between CLI and GUI.** `primer-cli/src/main.rs::probe_espeak_ng_data` and `primer-gui/src/lib.rs::probe_espeak_ng_data` carry byte-identical logic except for the log channel. Low-priority — move shared impl to `primer-speech` if either side needs to diverge.

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

Newly carried into this brief from PR #121:

- **The append separator is a single space, not a punctuation cue.** Whisper transcripts typically include surrounding punctuation in the segment text (`. ! ?` are common at sentence boundaries) but the `trim()` step in `state_machine.rs` strips outer whitespace from both halves before append, so a sentence like `"why does."` + `"the sky look blue?"` would stitch as `"why does. the sky look blue?"` — the inner whitespace around the period is preserved by the single-space append. This matches how a human transcriber would render a paused sentence. If a future fix wants to drop the cue (e.g. stitch `"why does"` + `"the sky look blue"` as `"why does, the sky look blue"`), the append site is the right place.
- **`ScriptedStreamingStt` is intentionally minimal.** It drains a FIFO of finalize texts; it doesn't model partial pushes via `push_audio`, doesn't model session-level errors, and doesn't model latency. The mock is exactly sized to test the cancel-retry path; broader STT behaviour testing would want a richer mock.
- **The `if !new_text.is_empty()` gate around the append matters.** Without it, an iteration whose `finalize()` returned an empty segment list would append a phantom trailing space, and the `if transcript_so_far.is_empty()` empty-check below would still see a non-empty (single-space) string and skip the break. Pinned by the scripted-mock sanity test indirectly (the third pop returns empty); not pinned by a dedicated test today. If a future refactor moves the append, the empty-gate must move with it.

Carried over from PR #120's brief, still pending:

- **Backdrop-click-closes is preserved for settings, intentionally NOT added for voice-consent.** The original settings modal closed on backdrop click; the voice-consent modal did not. The migration preserved both behaviours. If a future spec wants the voice-consent modal to close on backdrop click too, add a `dialog.addEventListener("click", ...)` block mirroring the settings.js pattern — gated on whatever state should block backdrop-close (e.g. an in-flight download).
- **Escape on the voice-consent modal now routes through `onCancel` (newly enabled by `<dialog>`'s native cancel event).** Previously Escape did nothing on this modal — there was no keyboard dismiss path. Native `<dialog>` makes Escape close-by-default; the migration routes it through `onCancel` so `stop_voice_mode` still fires.
- **The HTML-tag walker `tag_for_id` is intentionally simple.** Walks back from `id="..."` to the nearest `<` and reads the next alphanumeric run. Three unit tests pin the basic cases. It would mis-handle e.g. `<![CDATA[ id="x" ]]>` (the `[` characters interrupt the alphanumeric run incorrectly) — not a real concern in our HTML, but worth noting if the codebase ever ingests user-authored markup.
- **`dialog.modal[open] { display: flex; ... }` is the load-bearing CSS rule.** Bare `display: flex` on `dialog.modal` would conflict with the UA's `dialog:not([open]) { display: none; }`. The `[open]` selector lets the flex layout kick in only when the dialog is actually shown, without needing `!important` to override `display: none`.
- **The `cancelListener` removal in voice.js cleanup() is load-bearing.** Without it, a re-opened consent modal would leak listeners and `stop_voice_mode` would fire twice on the second close. Pinned by code review; not pinned by a test today. If a future refactor of voice.js moves the listener wiring, the cleanup path must move with it.

Carried over from PR #119's brief, still pending:

- **CSP regression test reads `tauri.conf.json` via `include_str!`** — that's the right mechanism (compile-time embed of the file shipped) but the file location is hard-coded to `../tauri.conf.json` relative to `src/csp.rs`. A future restructure that moves `tauri.conf.json` would break compilation loudly (which is fine), but a structural reorganisation of the crate would need to update the relative path. **Same caveat applies to `modal_dialog_contract.rs`** — it `include_str!`s four UI assets from `../ui/`. A crate restructure must update those paths in lockstep.
- **The `' * '` wildcard check was considered but dropped from `FORBIDDEN_CSP_KEYWORDS`** because the surrounding-space-required pattern is brittle. The narrower assertion bar (only `'unsafe-inline'` and `'unsafe-eval'`) is intentional.
- **No object-form CSP migration** — the string form remains short enough that the cost-benefit hasn't tipped.

Carried over from PR #118's brief, still pending:

- **`SpeechLoopConfig` shape now differs between speech builds.** On macOS-native it has three fields; on every other speech build it has seven. Any future code that introspects this struct (serialization, debug-formatting, builder pattern, etc.) needs matching cfg gates.
- **Owned `PathBuf` / `String` in `SpeechLoopConfig` means one extra clone per Path on the non-native build.** This runs once at session start; negligible.
- **`Cli` struct field set now varies by build.** Future tests that hardcode field counts via reflection would need cfg gates; current tests don't.

Carried over from PR #117's brief, still pending:

- **Open-counter is thread-local, not global.** The `session_store_open_count` test seam relies on `#[tokio::test]`'s default `current_thread` flavour.
- **`SqliteSessionStore::set_locale` is mutable by reference.** Future code that holds the store as `Arc<dyn LearnerStore>` cannot call it.
- **`__concept_language_tag_for_tests` opens a sibling `rusqlite::Connection`.** Silently returns `None` on open failure rather than panicking.

**Manual real-LLM smoke for Hindi and OpenAI-compat has not run.** Same recommendation as the prior brief:

- Hindi: `~/.cargo/bin/cargo run --bin primer -- --backend cloud --language hi --name Aarav --age 9 --no-persist --verbose`.
- OpenAI-compat: spin up `oMLX --serve` or `llama-server --port 8000 --model <gguf>`; `~/.cargo/bin/cargo run --bin primer -- --backend openai-compat --openai-compat-url http://localhost:8000 --model <model> --no-persist --verbose`.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing.)

- **Smallest correct patch over the principled refactor when the trait surface is fixed.** Issue #103 had three fix options. Option 1 (`peek()` on `StreamingSpeechToText`) was the principled long-term shape but required an upstream-trait API expansion (`process_step` on whisper-cpp-plus) plus cascade through every backend including mocks. Option 2 (accumulate on append) was a single-site consumer change with the same observable behaviour for both the mock and production. Option 2 lands now; option 1 is preserved as a follow-up for when the upstream trait grows. The general principle: when the trait can't change today, pick the consumer-side fix that doesn't paint future code into a corner — accumulate is reversible, a `peek()` rip-out wouldn't be.
- **Scripted mocks for state machines where behaviour differs across calls.** When testing a state machine that loops over a trait method whose return value changes per call (e.g. `open_session()` returning sessions with different finalize text on each iteration), a single-shot mock (returning the same canned text every time) is structurally unable to exercise the iteration-sensitive bug. A FIFO-draining scripted mock is the smallest possible expansion: takes a `Vec<T>` of expected returns, pops one per call. Out-of-script returns to a benign default (here: empty string) rather than panicking, so the test fails on the substantive assertion rather than on mock exhaustion. Worth using whenever a regression hinges on "what if successive calls return different things?".
- **Pin a static config-file contract via `include_str!` + a `#[cfg(test)]` parser test, not a runtime check** (issues #71 and #81).
- **Prefer browser-supplied behaviour over hand-rolled equivalents when the platform offers it.** Native `<dialog>` + `showModal()` (issue #81).
- **Static-analysis sweep before tightening a security policy.** (issue #71).
- **Cfg-gate CLI fields + the matching struct fields together, never just one side** (issue #112).
- **Drop lifetimes from cfg-gated structs by owning their references.**
- **`#[cfg_attr]` to switch a single attribute payload, not just enable/disable an attribute.**
- **`#[doc(hidden)] pub` cross-crate test seams in `primer-storage`** (issues #87 + #116).
- **Pin the on-disk consequence, not just the in-memory inputs** (issue #116).
- **Thread-local counters as test seams for behavioural pin tests** (issue #86).
- **Reorder construction to fold redundant probes into the build path** (issue #86).
- **`set_locale`-style re-tag methods when the resource itself is locale-neutral** (issue #86).
- **Opt-in version-controlled git hooks under `.githooks/`.**
- **CI as source of truth; local hooks as early-warning copies.**
- **Resolve binary tools via $ENVVAR → known install path → PATH.**
- **Single source of truth at the IPC trust boundary** (PR #108).
- **Verify before claiming closed.**
- **Co-locate workflow-level policies with the steps that enforce them.**
- **TDD-driven validator extension.**
- **Subagent-driven development with two-stage review (spec + code-quality) per task.**
- **Promote modules that have outgrown their original location.**
- **Two-firewall preview gates for safety-critical opt-outs.**
- **In-process `tokio::net::TcpListener` for HTTP behavior tests.**
- **Borrowed client / `FnMut` callback test seam for async streaming.**
- **Pack-side i18n for any locale-keyed display string the GUI surfaces.**
- **Server-side re-resolution at IPC trust boundaries.**
- **Shared test harness with `*Config` carrier struct + locale-specific shim.**
- **Pure functions in dedicated modules** for algorithmic cores — tested directly without I/O.
- **`#[cfg(test)]` test seams with `pub` accessor methods** — zero production cost.
- **Locale-keyed templates in TOML packs** with `{placeholder}` substitution at trait method level.
- **Carrier structs with `disabled()` no-op constructors** for parameter bundles that not all callers need to configure.
- **Lookup-table seeding for new closed-enum variants** — no schema migration needed.
- **Constants in `consts.rs` submodules** (Rust) or top-of-module `_DEFAULT_*` constants (Python).
- **TDD discipline.** Tests first; watch them fail; implement to green.
- **File-size hygiene.** Keep modules under 500 lines where reasonable.
- **Network-injection test seam** for any data-ingest pipeline.
- **Defensive sanity tests at the data layer.**
- **Always pin `Default` impls of public structs to `consts::*`, with a drift-prevention test.**
- **Pure helpers + their own unit tests, even in `#[ignore]`'d integration tests.**
- **Frozen dataclasses for process-wide configuration.**
- **String discriminators for strategy selection, with allow-list validation.**
- **Re-run the live ingest after changing any fetch-path helper.**
- **Kwarg-injected side-effect functions for TDD seams in Python.**
- **Structural ingest-time defences beat manual probing habits.**
- **Plan-then-execute inline for mechanical refactors with a strong test safety net.**
- **Two-commit refactor: "set up the change" then "remove the old".**
- **Ship the follow-up issue body with explicit acceptance + per-file checklist when a PR ships a transitional state.**
- **Per-locale dataset modules under `tests/common/<pack>.rs`** for benchmark data.
- **Locale-specific cluster sizing via a named const** (`DE_CLUSTERS = 5`).
- **Required-loose-substrings drawn from the canonical article's lead, not the query phrasing.**

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer
git status                       # confirm clean state
git fetch --all --prune

# Check PR #121 status (pre-merge: still open on voice/cancel-retry-transcript-stitch-issue-103).
gh pr view 121
gh pr checks 121

# If PR #121 has merged, switch back to main and pull.
git checkout main && git pull
# Then optionally delete the merged branch locally and on origin:
git branch -D voice/cancel-retry-transcript-stitch-issue-103 2>/dev/null || true
git push origin --delete voice/cancel-retry-transcript-stitch-issue-103 2>/dev/null || true

# If PR #121 is still open, check it out to run the manual voice-mode smoke locally:
git checkout voice/cancel-retry-transcript-stitch-issue-103
git log --oneline -3             # 9a17aa7 on top; f2fae07 (PR #120) below it on main

# Check for any new PRs or issues opened since this brief.
gh pr list --state open
gh issue list --state open --limit 30

# Opt-in to the local pre-commit hook (one-time per clone; from PR #109):
git config core.hooksPath .githooks

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 856 passed, 0 failed, 3 ignored (default features).

~/.cargo/bin/cargo test -p primer-speech --features voice-loop
# Expected: 84 passed, 0 failed, 2 ignored.

~/.cargo/bin/cargo test -p primer-cli --features speech
# Expected: 12 passed, 0 failed, 0 ignored.

# On macOS only:
~/.cargo/bin/cargo test -p primer-cli --features speech,macos-native
# Expected: 12 passed, 0 failed, 0 ignored.

~/.cargo/bin/cargo test -p primer-gui --features speech
# Expected: 146 passed, 0 failed, 0 ignored.

~/.cargo/bin/cargo fmt --all -- --check
# Expected: clean.

~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
# Expected: clean exit 0.

RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo test --workspace --no-fail-fast
# Expected: 856 passed.

RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-gui/speech
# Expected: clean exit 0 (the speech-features build is not yet on CI; verify locally).
```

To do a manual voice-mode smoke verifying the cancel-retry fix (recommended before merging #121):

```bash
cd /Users/hherb/src/primer/src

# CLI --speech mode (needs whisper + piper assets staged):
~/.cargo/bin/cargo run --features primer-cli/speech --bin primer -- \
    --speech --name SmokeTester --age 9 --no-persist --verbose \
    --whisper-model <path> --voice-onnx <path>.onnx --voice-config <path>.onnx.json \
    --voice <model-id>
# Then speak a sentence with an intentional mid-sentence pause, e.g.:
#   "why does the sky look blue and... why does it sometimes look red?"
# The pause should be long enough for silero to trip SpeechEnd, then your
# resumption should trip SpeechStart inside the LATENT_THINK window.
# Expected: the [stt] verbose line for the COMMIT phase shows the FULL
# stitched utterance, not just the post-pause tail. Pre-fix it showed only
# the tail.

# GUI voice mode (Tauri):
~/.cargo/bin/cargo run -p primer-gui --features speech
# Toggle Voice mode, speak a paused sentence, observe the bubble: the
# transcript displayed must be the full utterance, not just the tail.
```

To exercise the macOS-native build manually (verifies #112's fix is still in force):

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo run --features primer-cli/speech,primer-cli/macos-native --bin primer -- \
    --speech --name SmokeTester --age 9 --no-persist --verbose
# Expected: no clap MissingRequiredArgument error — the four whisper/piper
# flags are no longer required (or even declared). SFSpeechRecognizer +
# AVSpeechSynthesizer carry STT and TTS; Silero stays as the VAD.
```

To exercise the Hindi preview locale manually:

```bash
cd /Users/hherb/src/primer/src
RUST_LOG=primer::prompt_pack=warn,info ~/.cargo/bin/cargo run --bin primer -- \
    --backend stub --name Aarav --age 8 --language hi --no-persist
# Expected: one WARN line "prompt pack is in preview status — machine-translated content
# awaiting native-speaker review ... locale=hi" before the first turn.
```

For a real-LLM Hindi smoke (recommended before flipping to stable):

```bash
cd /Users/hherb/src/primer/src
ANTHROPIC_API_KEY=... RUST_LOG=info ~/.cargo/bin/cargo run --bin primer -- \
    --backend cloud --name Aarav --age 9 --language hi --no-persist --verbose 2>&1 | tee /tmp/smoke_hi.log
```

For an OpenAI-compat smoke (spin up a local server first, e.g. llama-server):

```bash
# In one terminal:
llama-server --port 8000 --model /path/to/some.gguf

# In another:
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo run --bin primer -- \
    --backend openai-compat --openai-compat-url http://localhost:8000 \
    --model <model-id-from-server> \
    --name SmokeTester --age 9 --no-persist --verbose
```

To re-run the German regression benchmarks (both flavours; unchanged this session):

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_de
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_hybrid_de
# Real BGE-M3 recall floor (downloads ~570 MB on first run; cached afterwards):
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test retrieval_quality_hybrid_de
```

For the Python ingestion pipeline tests (uv-only — never invoke pip directly):

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/pytest tests/
# Expected: 135 passed.
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- If you discover that Horst did interim work that changes the plan, flag it. Always verify open-issue claims and `git log origin/main` since this brief's "last updated" timestamp before starting.
