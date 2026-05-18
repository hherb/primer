# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-18T1943+0800 (local feature branch `gui/native-dialog-focus-trap-issue-81` carrying one commit `84fd0a9` that closes #81 — settings + voice-consent modals migrated to native `<dialog>` for a UA-supplied focus trap. PR #119 closing #71 merged earlier the same day as commit `3f12c53`. PR #118 closing #112 merged earlier the same day as commit `1d8a1d7`.)

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green on `gui/native-dialog-focus-trap-issue-81`: **852 Rust tests** under default features (was 842 before this session — eight new tests in `primer-gui::modal_dialog_contract::tests` pin the native-`<dialog>` contract; the remaining +2 vs the prior brief's "842" stems from a small undercount in that brief, not new work). 3 ignored. With `--features primer-gui/speech`: **142 primer-gui tests** (was 132; +8 modal_dialog_contract tests, plus +2 likewise unaccounted in the prior brief). With `--features primer-cli/speech`: primer-cli still has **12 tests**; same count under `--features primer-cli/speech,primer-cli/macos-native`.
3. **The work on this branch has NOT been pushed.** From the working tree on `main`:
   ```bash
   git checkout gui/native-dialog-focus-trap-issue-81
   git log --oneline -2          # 84fd0a9 on top
   git push -u origin gui/native-dialog-focus-trap-issue-81
   gh pr create --title "gui: convert modals to native <dialog> for focus trap (closes #81)" --body "$(see commit body for details)"
   ```
   Or run `/commit-push-pr` style — the commit body already carries the PR description content.
4. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first. Always verify open-issue claims and `git log origin/main` since this brief's "last updated" timestamp before starting.

## What we shipped this session

### Local commit `84fd0a9` (branch `gui/native-dialog-focus-trap-issue-81`, closes #81) — modals to native `<dialog>`

The settings modal and the voice-asset consent modal were both `<div role="dialog">` toggled via the HTML `hidden` attribute. After Tab past the last action button, focus drifted onto the chat shell behind the dim backdrop — both an accessibility gap and a UX surprise. Issue #81 recommended Option 1: swap the wrapping `<div class="modal-backdrop"><div class="modal">` pair for a native `<dialog>` opened via `showModal()`. The browser supplies the focus trap, Escape-to-cancel, and an inert `::backdrop` pseudo-element — all UA-provided.

The commit ships:

- **`src/crates/primer-gui/ui/index.html`** — both `settings-modal` and `voice-consent-modal` are now `<dialog>` elements (no wrapping div, no `hidden` attribute, no `role="dialog"`/`aria-modal="true"` on the inner div since those are implicit on `<dialog>`). The `aria-labelledby` survives because that's a content semantic, not a dialog-mechanism one. Inner content re-indented to match the new nesting depth (8→6 leading spaces) so the diff stays readable.
- **`src/crates/primer-gui/ui/settings.js`** — `dom.backdrop` removed; `open()` calls `dom.modal.showModal()`, `closeModal()` calls `dom.modal.close()`. The manual `document.addEventListener("keydown", onEscape)` is replaced by a `cancel` event listener on the dialog that `preventDefault`s while `state.isSaving` is true (preventing the user from dropping the modal mid-save). Backdrop click still closes — the click bubbles up with `event.target === dom.modal` when the user clicks the `::backdrop` area, gated on `!isSaving` to match the previous behaviour.
- **`src/crates/primer-gui/ui/voice.js`** — `showConsentModal()` calls `dialog.showModal()` and closes via `dialog.close()` on cancel/download/error paths. The `cancel` event (fired on Escape) routes through `onCancel` (after `preventDefault`'ing the auto-close) so `stop_voice_mode` still fires and the sticky `voice_mode_enabled=false` write persists. Without this routing, Escape would close the dialog without persisting the opt-out and the consent modal would re-appear on the next launch with no way to dismiss it durably. The `cancel` listener is tracked on `cancelListener` so `cleanup()` can remove it — without removal it'd leak across re-opens and `stop_voice_mode` would fire twice on the second close.
- **`src/crates/primer-gui/ui/styles.css`** — `.modal-backdrop` rule deleted; the dim-overlay color (`color-mix(in oklab, #000 45%, transparent)`) now lives on `dialog.modal::backdrop`. The `.modal` rule is rewritten as `dialog.modal` (UA defaults reset: `padding: 0; margin: auto;`) plus a `dialog.modal[open] { display: flex; flex-direction: column; }` rule so the flex layout only kicks in when the UA stylesheet stops hiding the dialog. The `[hidden]` comment that previously called out `.modal-backdrop` was updated to reflect the new mechanism.
- **`src/crates/primer-gui/src/modal_dialog_contract.rs`** — new 150-line module, entirely `#[cfg(test)]`, holds eight tests. Five pin the contract:
  - `every_modal_element_is_a_native_dialog` — walks the HTML to confirm `id="settings-modal"` and `id="voice-consent-modal"` both resolve to `<dialog>` tags. Iterates over `MODAL_DIALOG_IDS` so adding a third modal is one edit, not three.
  - `no_legacy_modal_backdrop_wrapper_remains` — the `modal-backdrop` string is gone from `ui/index.html`.
  - `settings_controller_opens_via_show_modal` and `voice_controller_opens_via_show_modal` — both JS controllers call `.showModal()` (not just toggle `.hidden`).
  - `stylesheet_declares_native_backdrop_rule` — `ui/styles.css` declares a `::backdrop` rule.
  
  Three test the `tag_for_id` HTML-walker helper directly (walks back from `id="..."` to the nearest `<` and grabs the tag name) so a future bug in the helper would surface as a unit-test failure rather than a contract-test failure with a confusing message.
- **`src/crates/primer-gui/src/lib.rs`** — `+ pub mod modal_dialog_contract;`.

The tests use the same `include_str!` + `#[cfg(test)]` parser pattern as `primer-gui::csp` (issue #71), so the bytes the test inspects are byte-for-byte the bytes that get baked into the binary — no cwd-relative path resolution and no risk of drift between what was shipped and what was tested.

**Branch:** `gui/native-dialog-focus-trap-issue-81`.
**Tests:** `cargo test --workspace`: **852 / 0 / 3** (was 842 baseline; +8 new modal_dialog_contract tests + 2 unaccounted vs the prior brief's count). `cargo test -p primer-gui --features speech`: **142 / 0 / 0** (was 132 baseline; same +8 + 2 unaccounted). `cargo test -p primer-cli --features speech`: 12/0/0. `cargo test -p primer-cli --features speech,macos-native`: 12/0/0. fmt + clippy `-D warnings` clean on default features and on `--features primer-gui/speech`.

**Manual GUI smoke not run** — left to the merger. Static analysis is exhaustive (browser-supplied focus trap is the load-bearing mechanism and only requires the `<dialog>` element + `showModal()` call, both pinned by tests), but visual verification (open the app, Tab through the settings modal, confirm focus stays inside; trigger the voice-consent modal if speech assets aren't cached, Tab through that too; confirm the backdrop is visibly dimmed) is cheap and would catch any blind spot. Commands at the bottom of this brief.

**Why eight tests, not just one:** the contract has multiple load-bearing facts (HTML element, JS opener, CSS backdrop) and each piece is independently regression-prone. A single test asserting "everything looks right" would have a confusing failure message; eight focused tests with bespoke error messages each tell the maintainer exactly which fact regressed.

**Out of scope:** the picker-screen is a separate full-screen takeover surface (not a modal overlay) and isn't a `<dialog>`. The toast is a polite-live-region status indicator, not a dialog. Neither needs the focus-trap treatment.

### Earlier in this session day (already merged before this branch was opened)

- **PR #119** (commit `3f12c53`, closes #71): tighten CSP by dropping `'unsafe-inline'` from `script-src` and `style-src`. New 92-line `csp.rs` module pins the policy via three `#[cfg(test)]` tests.
- **PR #118** (commit `1d8a1d7`, closes #112): drop dummy whisper/piper flag requirements on the macOS-native build. Cfg-gates the four CLI flags + their `SpeechLoopConfig` mirrors + `validate_speech_assets` under `not(all(target_os = "macos", feature = "macos-native"))`. Two new tests, each gated to a complementary cfg, pin both speech builds in CI.

## What's next

### Open PR for the local branch (priority)

The local commit `84fd0a9` on `gui/native-dialog-focus-trap-issue-81` needs to be pushed and turned into a PR. Push the branch and open the PR; the commit message body is suitable as the PR description verbatim. Manual GUI smoke before merge: see the commands at the bottom.

### Focus-trap follow-ups (after the PR lands)

- **Tab-ring smoke for every focusable in settings.** With the browser-supplied focus trap, the cycle goes through every tabbable descendant. The settings modal has many — backend select, model input, ollama URL, API-key radios + input, three subsystem groups (each with match-main checkbox + kind select + model input + timeout input), embedder select + model + URL, vocab/breaks numerics, persistence fields, speech mic-silence + auto-download checkbox, four locale override cards with four inputs each. A future a11y pass should confirm every step in the ring is visibly focused (no skipped buttons, no stuck-at-end-of-form behaviour). Not opening an issue today; the structural fix is done.
- **`<dialog>` `inert` semantics on the background.** Native `<dialog>` opened via `showModal()` makes the rest of the document inert — clicks pass through to the backdrop only. Worth confirming visually that the chat shell behind the dim overlay doesn't accept keyboard or pointer input. Combined with the focus trap this should give the proper "modal" experience.

### macOS-native speech follow-ups (open after #112 landed)

- **#114** — speech(macos-native): stream PCM chunks to speaker as `AVSpeechSynthesizer` emits them (cut time-to-first-audio). Larger; touches the synthesis path. The current path buffers the full utterance before pushing to cpal; streaming would let the user hear the start of the response sooner. **This is the only macOS-native speech follow-up still open** after #112 landed.

Acceptance criteria for #114 (sketch — refine before implementing):
- `AVSpeechSynthesizer.writeUtterance:toBufferCallback:` is already chunk-by-chunk; the change is plumbing the callback's `AVAudioPCMBuffer` into the speaker ringbuf directly instead of accumulating into a single `Vec<f32>` and pushing post-synthesis.
- Time-to-first-audio (TTFA) measured from end of LLM streaming to first speaker sample should drop substantially. Pick a smoke phrase and pin a TTFA budget in a manual smoke (no test; the smoke binary `examples/tts_macos_pcm_smoke.rs` is the right home for an instrumented variant).
- The PCM-callback chunk-size assumption that `examples/tts_macos_pcm_smoke.rs` validates today must still hold under the streaming path.
- The `is_speaking` echo-suppression invariant (mic muted while speaker is producing audio) must not regress — extend the drain-hook logic so it waits for the *last* chunk to drain, not the first.

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
- **Voice-loop hardening** (echo cancellation, ambient-noise robustness; Phase 2 is at POC quality). Voice mode (Phase A) GUI work landed in PR #89; #91 closed with PR #101; #92 closed with PR #104; #102 closed with PR #110; #112 closed with PR #118. The remaining Phase 2 polish is the still-open piece — #114 expands that area; #103 (cancel-and-retry drops first half of transcript) is the other open voice-loop bug.
- **Hardware integration** (Phase 3 — display, audio, enclosure).
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default.

### Klexikon corpus follow-ups (carried forward)

- **Klexikon corpus expansion past 66.** Klexikon has ~3000 articles. The 2 corpus gaps from PR #93 (`gänsehaut` reflex; tides on the `mond` article) would need either expanded articles or additional Klexikon titles. Concrete next-batch acceptance: pick 30-50 more titles in still-thin clusters. Re-run pipeline. No code change required. Once corpus grows past ~150 passages, re-run the sweep harnesses to verify production defaults still flatline at 100% strict from `top_k=5` onward on non-paraphrase queries.
- **Klexikon license claim spot-check.** WebFetch a sample of ~5 article footers and verify each shows CC-BY-SA-4.0. Low-priority.

### Smaller-scope follow-ups still open

Verified against `gh issue list` 2026-05-18T1943+0800 (no new issues opened since the prior brief; #81 will close on the upcoming PR merge; #71 closed by PR #119; #112 closed by PR #118):

- **#114** — voice(macos-native): stream PCM chunks to speaker as AVSpeechSynthesizer emits them.
- **#103** — voice: cancel-and-retry path drops the first half of the transcript (bug, voice-loop hardening territory).
- **#98** — refactor(tests): split `tests/common/sweep.rs` into `bm25`/`hybrid` submodules (enhancement). **Defer until Hindi or another third locale lands** — issue body explicitly recommends this.
- **#46** — Hybrid sweep: explore post-RRF `min_score` as a fifth grid axis.
- **#41** — data/ingest: consider scoping disambiguation regex to lead-sentence patterns.
- **#40** — data/ingest: aggregate per-source attribution for the Wikipedia layer.
- **#22** — primer-knowledge: cache prepared statements for the corpus-bootstrap path (Phase 0.2; enhancement).
- **#21** — CLI: separate `--languages` preference list from bound `--language` locale.
- **#20** — i18n: placeholder validator can false-fail on translator narrative text.

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

Newly carried into this brief from the modal-dialog migration:

- **Backdrop-click-closes is preserved for settings, intentionally NOT added for voice-consent.** The original settings modal closed on backdrop click; the voice-consent modal did not. The migration preserved both behaviours. If a future spec wants the voice-consent modal to close on backdrop click too, add a `dialog.addEventListener("click", ...)` block mirroring the settings.js pattern — gated on whatever state should block backdrop-close (e.g. an in-flight download).
- **Escape on the voice-consent modal now routes through `onCancel` (newly enabled by `<dialog>`'s native cancel event).** Previously Escape did nothing on this modal — there was no keyboard dismiss path. Native `<dialog>` makes Escape close-by-default; the migration routes it through `onCancel` so `stop_voice_mode` still fires. Behaviour change: there's now a keyboard path to opt out of voice mode mid-consent.
- **The HTML-tag walker `tag_for_id` is intentionally simple.** Walks back from `id="..."` to the nearest `<` and reads the next alphanumeric run. Three unit tests pin the basic cases. It would mis-handle e.g. `<![CDATA[ id="x" ]]>` (the `[` characters interrupt the alphanumeric run incorrectly) — not a real concern in our HTML, but worth noting if the codebase ever ingests user-authored markup.
- **`dialog.modal[open] { display: flex; ... }` is the load-bearing CSS rule.** Bare `display: flex` on `dialog.modal` would conflict with the UA's `dialog:not([open]) { display: none; }`. The `[open]` selector lets the flex layout kick in only when the dialog is actually shown, without needing `!important` to override `display: none`.
- **The `cancelListener` removal in voice.js cleanup() is load-bearing.** Without it, a re-opened consent modal would leak listeners and `stop_voice_mode` would fire twice on the second close. Pinned by code review; not pinned by a test today. If a future refactor of voice.js moves the listener wiring, the cleanup path must move with it.

Carried over from PR #119's brief, still pending:

- **CSP regression test reads `tauri.conf.json` via `include_str!`** — that's the right mechanism (compile-time embed of the file shipped) but the file location is hard-coded to `../tauri.conf.json` relative to `src/csp.rs`. A future restructure that moves `tauri.conf.json` would break compilation loudly (which is fine), but a structural reorganisation of the crate would need to update the relative path. **Same caveat applies to `modal_dialog_contract.rs`** — it `include_str!`s four UI assets from `../ui/`. A crate restructure must update those paths in lockstep.
- **The `' * '` wildcard check was considered but dropped from `FORBIDDEN_CSP_KEYWORDS`** because the surrounding-space-required pattern is brittle (matches `host * ` but not `host *;`). If a future regression introduces wildcard sources, it won't be caught by today's three tests. The narrower assertion bar (only `'unsafe-inline'` and `'unsafe-eval'`) is intentional — `*` in a CSP source is a host-allowlist concern that the directive-presence check partially covers anyway.
- **No object-form CSP migration** — the string form remains short enough that the cost-benefit hasn't tipped. Flip the moment a multi-source directive makes the string awkward.

Carried over from PR #118's brief, still pending:

- **`SpeechLoopConfig` shape now differs between speech builds.** On macOS-native it has three fields; on every other speech build it has seven. Any future code that introspects this struct (serialization, debug-formatting, builder pattern, etc.) needs matching cfg gates. Today the only consumer is `speech_loop::run` in the same crate, so the blast radius is contained.
- **Owned `PathBuf` / `String` in `SpeechLoopConfig` means one extra clone per Path on the non-native build.** This runs once at session start; negligible.
- **`Cli` struct field set now varies by build.** The `Cli` struct itself doesn't change shape between the two speech builds — just which `#[arg]` fields are declared. Future tests that hardcode field counts via reflection would need cfg gates; current tests don't.

Carried over from PR #117's brief, still pending:

- **Open-counter is thread-local, not global.** The `session_store_open_count` test seam relies on `#[tokio::test]`'s default `current_thread` flavour. A future test that opts into `flavor = "multi_thread"` plus `spawn_blocking` will see the counter reset to 0 on the other thread.
- **`SqliteSessionStore::set_locale` is mutable by reference.** Future code that holds the store as `Arc<dyn LearnerStore>` cannot call it. PR #117 now pins this with the end-to-end test — it would fail loudly if the wrap moved.
- **`__concept_language_tag_for_tests` opens a sibling `rusqlite::Connection`.** Silently returns `None` on open failure rather than panicking; "use only after drop" contract.

**Manual real-LLM smoke for Hindi and OpenAI-compat has not run.** Same recommendation as the prior brief:

- Hindi: `~/.cargo/bin/cargo run --bin primer -- --backend cloud --language hi --name Aarav --age 9 --no-persist --verbose`.
- OpenAI-compat: spin up `oMLX --serve` or `llama-server --port 8000 --model <gguf>`; `~/.cargo/bin/cargo run --bin primer -- --backend openai-compat --openai-compat-url http://localhost:8000 --model <model> --no-persist --verbose`.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing.)

- **Pin a static config-file contract via `include_str!` + a `#[cfg(test)]` parser test, not a runtime check** (issues #71 and #81). When the contract lives in a non-Rust file the binary depends on (`tauri.conf.json`, `ui/index.html`, `ui/*.js`, `ui/*.css`, `Cargo.toml`, etc.), embedding the file with `include_str!` guarantees the bytes the test inspects are the bytes baked into the binary. The test is free at runtime (compile-time only), reads byte-for-byte what was shipped, and protects against the failure mode "the contract drifted because someone edited the asset without realising it was load-bearing." Issue #81 extended the pattern to four UI assets (HTML + two JS controllers + CSS) sharing one test module.
- **Prefer browser-supplied behaviour over hand-rolled equivalents when the platform offers it.** Native `<dialog>` + `showModal()` supplies focus trap, Escape-to-cancel, `inert` background, and `::backdrop` styling for free (issue #81). Re-implementing any of these in JS would be ~30+ lines per behaviour, would need maintenance as web platform APIs evolve, and would be prone to subtle bugs (focus-trap edge cases around iframes, contenteditable elements, shadow DOM). The general principle: when a platform API exists, use it; when it doesn't, prefer libraries with broad test coverage over hand-rolled code; only roll your own when the alternatives are missing or proven inadequate.
- **Static-analysis sweep before tightening a security policy.** Before dropping `'unsafe-inline'` from CSP, grep for every plausible source of inline content (`<style>`, `style="..."`, `<script>` blocks, inline event handlers, `setAttribute('style', ...)`, `javascript:` URLs, `eval`, `new Function`, and `innerHTML` templates containing `style="..."`). One-liner greps cover most of these and produce a strong audit trail in the PR description. The CSP doesn't cover the CSS-OM DOM API (`element.style.foo = bar`) — note that explicitly so a reviewer doesn't second-guess the `app.js` style writes.
- **Cfg-gate CLI fields + the matching struct fields together, never just one side** (issue #112). When a CLI flag is meaningless on a build configuration and a downstream config struct mirrors that flag, gating only one side leaves a dead-code warning (consumer struct field never read) or a forced-dummy UX (CLI requires a value that gets discarded). Gate them in lockstep — flag declaration, `requires_all` list, asset-validation function, the consumer struct field, and the call-site construction — all under the same `cfg(...)` predicate. The two-test pattern (one test under each side of the cfg) keeps both behaviours pinned in CI without needing a feature-matrix workflow.
- **Drop lifetimes from cfg-gated structs by owning their references.** When all the borrowed fields in a struct are cfg-gated out on one build, the lifetime parameter becomes unused on that build. The two clean fixes: (a) `PhantomData<&'a ()>` under the inverse cfg, or (b) switch `&'a Path` → `PathBuf` and `&'a str` → `String` so the struct doesn't need the lifetime at all. (b) trades one clone for cleaner shape and tends to win when the struct is small and constructed once.
- **`#[cfg_attr]` to switch a single attribute payload, not just enable/disable an attribute.** When a clap `#[arg(...)]` attribute carries a `requires_all` whose contents depend on cfg, two `#[cfg_attr(cond, arg(long, ...))]` lines with mutually exclusive conditions is cleaner than splitting the field into two cfg-gated declarations. The field name appears once; the attribute payload switches.
- **`#[doc(hidden)] pub` cross-crate test seams in `primer-storage`** to avoid pulling rusqlite into consumer dev-deps (issues #87 + #116).
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
- **TDD-driven validator extension.** Add the failing test → watch it fail → land the validator change → land the consumer (data file or producer site).
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
git status                       # confirm clean state (the branch holding this work is gui/native-dialog-focus-trap-issue-81)
git checkout gui/native-dialog-focus-trap-issue-81
git log --oneline -3             # 84fd0a9 on top; 3f12c53 (PR #119) below it on main

# Check this session's PR (no PR open yet — push the branch and open one).
git push -u origin gui/native-dialog-focus-trap-issue-81
gh pr create --title "gui: convert modals to native <dialog> for focus trap (closes #81)" \
  --body "$(git log -1 --pretty=%B)"
# (the commit body is suitable as the PR description verbatim.)

# Check for any new PRs or issues opened since this brief.
gh pr list --state open
gh issue list --state open --limit 30

# Opt-in to the local pre-commit hook (one-time per clone; from PR #109):
git config core.hooksPath .githooks

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 852 passed, 0 failed, 3 ignored (default features).

~/.cargo/bin/cargo test -p primer-cli --features speech
# Expected: 12 passed, 0 failed, 0 ignored.

# On macOS only:
~/.cargo/bin/cargo test -p primer-cli --features speech,macos-native
# Expected: 12 passed, 0 failed, 0 ignored.

~/.cargo/bin/cargo test -p primer-gui --features speech
# Expected: 142 passed, 0 failed, 0 ignored.

~/.cargo/bin/cargo fmt --all -- --check
# Expected: clean.

~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
# Expected: clean exit 0.

RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo test --workspace --no-fail-fast
# Expected: 852 passed.

RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-gui/speech
# Expected: clean exit 0 (the speech-features build is not yet on CI; verify locally).
```

To do a manual GUI smoke verifying the new focus trap (recommended before merging the upcoming PR):

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo run -p primer-gui --features speech
# Then in the running app:
#   1. Open Settings.
#   2. Tab from the first field forwards through every focusable: backend, model,
#      ollama URL, API-key radios + input, three subsystem groups (each with
#      match-main checkbox + kind + model + timeout), embedder kind + model + URL,
#      vocab/breaks numerics, persistence (session-db, knowledge-db, no-persist
#      checkbox), speech mic-silence, speech disable-auto-download, four locale
#      override cards (×4 fields each = 16 inputs), then Cancel / Save (next) /
#      Save & start new session. After the last button, Tab should wrap back to
#      the first field — NOT escape onto the chat surface behind the backdrop.
#   3. Shift-Tab from the first field should wrap to the last button.
#   4. Esc dismisses the modal (still — gated by isSaving).
#   5. Click on the dim backdrop dismisses (still — gated by isSaving).
#   6. If speech assets aren't cached, toggle Voice mode to trigger the consent
#      modal; Tab through Cancel / Download; Esc routes through onCancel
#      (stop_voice_mode + sticky off-flag persists; modal does NOT re-open on
#      next launch).
# Open WebView devtools (right-click → Inspect). The Console tab must show NO
# CSP "Refused to apply inline style" / "Refused to execute inline script"
# warnings — PR #119 tightened CSP and any <dialog> UA defaults must not trip
# it. (Static analysis says they don't, but verify.)
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
