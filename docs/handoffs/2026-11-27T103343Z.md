# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-11-27 (UTC).

**Open against `main` (independent; any order):**
1. **#434** (`fix/kb-cache-cleanup-406`): closes #406.
2. **#435** (`fix/pointless-fallback-notice-352`): #352 item 2 (the issue stays open for items 1 and 4).
3. **#436** (`refactor/cli-main-split`): behaviour-preserving split of `primer-cli/src/main.rs`.

Plus the docs PR (`docs/next-session-2026-11-27`, from `main`) with this brief.

**Since the last brief:**
- **#431, #432 and the docs PR #433 are merged** (`main` at `3f99eea`). #430 and #429 are CLOSED.
- **The sweep and inline-test detector were clean at session start.**
- **Owner decisions taken this session:** #406 = "legacy delete + copy-seed"; #352 item 2 = option (a), same kind **and** model.

## What we shipped this session

> SHAs are branch-tip commits as pushed; squash-merge gives new ones on `main`.

### #434 — `9208049`: a new KB cache starts from an existing one; the legacy file is deleted (closes #406)
- **Bug:** since #401 each seed-corpus folder has its own cache. So a new worktree, a fresh clone, or an upgrade from the pre-#401 `<model>.db` re-embedded the whole corpus (~100 s on BGE-M3), and the old file was never deleted.
- **Fix** (`primer-engine/src/knowledge/cache.rs`, called from `open_prepared_kb`):
  - **Copy-seed:** a cache file that doesn't exist yet starts as a snapshot of the newest same-model cache. The top-up then re-embeds only the passages that differ and prunes the rest.
  - **The snapshot** is the new `primer_knowledge::snapshot_db`: the source is opened read-only and copied with `VACUUM INTO` into a per-process temp file, then hard-linked into place. The link fails if the cache appeared meanwhile, so a concurrent start is never clobbered.
  - **Legacy delete:** once a hashed cache is ready, the legacy `<model>.db` beside it (and its `-journal`) is deleted. By then it has already seeded the new cache.
  - **Both steps are soft** (they log a `warn!` on failure).
  - **Not done (owner decision):** other folders' caches are kept. README and CLAUDE.md say `~/.primer/kb/` is safe to delete.
- **Tests:**
  - 2 `snapshot_db` tests in `primer-knowledge`.
  - 3 end-to-end tests in `primer-engine/src/knowledge/tests.rs`: only the diff is embedded, the source is untouched, another model's cache is never the source, the legacy file seeds and is then deleted, and an unhashed cache keeps its own file.
  - 3 pure name-parsing tests.
- **Mutation-checked:** removing the copy-seed call fails 2 tests; removing the legacy delete fails 1.

### #435 — `ba38cc6`: an identical fallback is a `BackendNotice` (#352 item 2)
- **New notice:** `BackendNotice::FallbackIdentical { fallback, fallback_model }`, returned with a `Wrapped` build on **both** the fallback and the router paths, when the fallback has the same kind **and** the same resolved model.
  - A model-less leg (stub) matches on kind alone.
  - The same kind with a different model reports nothing.
  - It replaces the kind-only `tracing::warn!` on the fallback path; the router path had no check before.
- **The CLI and GUI already render notices generically** (`Warning:` line, `#backend-notice-banner`), so neither needed a code change.
- **Size split:** `BackendNotice`, its `Display` and `served_identity` moved verbatim to the new `wiring/notice.rs`, so `backend.rs` is 451 lines (from 542). The `pub` surface is unchanged.
- **Mutation-checked:** a kind-only rule fails 2 tests.

### #436 — `d628bc3`: `primer-cli/src/main.rs` split (1362 → 399)
- **New files:** `cli.rs` (the clap `Cli`, re-exported as `crate::Cli`), `setup/{inference,session,embedder}.rs` (one startup step each, returning `Inference` / `SessionSetup` / `Option<Arc<dyn Embedder>>`), `repl.rs` (the text loop).
- **Verified** with the usual recipe:
  - The anchors were asserted, and the pieces rebuild the original.
  - The multiset line diff shows only the wrappers, imports, `pub(crate)` and four intended body edits.
  - 24 tests before and after.
  - Clippy is clean on 8 feature arms, and a stub REPL smoke run behaves as before.

### Checks run
- **#434:** `cargo test -p primer-engine -p primer-knowledge -p primer-gui` (129 / 51 / 259); clippy + rustdoc on both crates; fmt.
- **#435:** `cargo test -p primer-engine -p primer-gui -p primer-cli`; clippy + rustdoc on `primer-engine`.
- **#436:** see above.
- **Not run locally:** the full workspace test pass and the Android cross-compile (CI runs both).
- **CI at close:** #434 green; #435 and #436 were still running.

### Per-session sweep
- **Inline-test detector:** clean.
- **Oversized production files:** `primer-speech/src/macos/stt.rs` 504, `primer-inference/src/cloud.rs` 503. `primer-cli/src/main.rs` is 399 after #436.
- **Local gotcha:** `primer-gui`'s build script panicked with a seed dir under an old session's scratchpad worktree (`…/wt427/data/seed`), a stale build-script artefact. `cargo clean -p primer-gui` fixed it (it removed 20 GiB).

## What's next (concrete acceptance criteria)

### 0. Owner review/merge: #434, #435, #436, then the docs PR
- **Acceptance:** CI green on each; after #434 merges, `gh issue view 406 --json state` shows CLOSED.
- **Overlaps:** #434 and #435 both edit CLAUDE.md, but different bullets. After the first merges, check the others still merge (`gh pr view <N> --json mergeable`).

### 1. Sweep + inline-test detector: re-run EVERY session
Commands are in the resume block.

### 2. Hear #427 (+ #431, #432) on real hardware (owner)
- **Desktop:** `cargo run -p primer-gui --features speech` → Voice mode → ask a two-sentence question. The first sentence should start ~1.2 s after you stop, while the bubble is still filling. Press Stop mid-reply: speech stops after the current phrase, and the next question still gets an answer (#423).
- **Also check:** "bye" ends voice mode with no reply. macOS-native CLI: still faster to first audio.
- **#431:** a reply with a blank line between paragraphs should pause there instead of running on.
- **#432:** mock coverage only (`tts_error_in_a_whole_reply_speak_reports_the_exit`). There is no easy on-device trigger: Piper loads the voice into memory at start, so changing the path or deleting the file mid-session never fails a running loop. If a real TTS error ever happens, the UI should leave voice mode with "Voice mode ended (error)".
- **Tune `MIN_THINK_HOLD`** (1.2 s) and `STREAM_TAIL_FLUSH_IDLE` (300 ms) if needed.
- **Android (#260 checklist):** per-phrase speak + session-held `speaking` flag.

### 3. #325 stage 4 (optional): `clear_speaker` hook
- `LoopBackends` gets an optional `clear_speaker` that empties the ringbuf, so a Stop silences at once.
- **Acceptance:** a mock test where a cancel between phrases calls `clear_speaker` before the drain hook. Owner call after item 2.

### 4. Manual first-run GUI check for PR #396 (owner)
`mv ~/.primer/kb ~/.primer/kb.bak`, launch the GUI, Start new → Save & start; the button should count "… 0 / 91" upward.

### 5. Other bugs (host-completable; TDD each)
- **#352 item 1:** a per-turn "served by" signal when a `FallbackBackend` / `RouterBackend` fails over mid-session. It needs a design note first: a turn-complete payload field plus a GUI pill or banner.
- **#352 item 4:** mislabelled classifier rows from before #363. Design note first; related to #316 (the id-format migration).
- **Older:** #315 (Hindi SOV prefixes), #316. Run `gh issue list`; read each before starting.

### 6. Production-code splits (owner-approved lane, all feature-gated, so dual-verify)
- `stt.rs` (504) and `cloud.rs` (503) are marginal; split only if an edit grows them.
- **Optional follow-up to #436:** `setup/inference.rs` and `setup/session.rs` still call `std::process::exit` on user errors. Returning `anyhow` errors would make them testable, but it changes the exit codes (2 for router errors).
- **Recipe that worked for #409 / #416 / #425 / #436:**
  1. In Python, slice the file at unique, asserted anchors.
  2. Assert that the pieces rebuild the whole file.
  3. `cargo clippy` lists the stale imports.
  4. Compare the code lines with a normalised diff (a multiset of stripped lines).
  5. Compare test counts with `--list | grep -c ': test$'` before and after.
  - Save the script to the scratchpad and chain follow-up edits with `&&`: a failed assert must stop them.
  - For a big `async fn` body, wrap each moved block in a `fn` that takes `&Cli` and returns a struct whose fields have the *same names* as the old locals. The caller then destructures it, and the rest of the body is unchanged.

### Carried / owner-or-hardware-gated
- **#260:** Android-voice on-device acceptance. Also check that a denied mic keeps the text session (#369), that a stop which times out (#421) leaves the next session's recognizer alive, and the #426/#427 per-phrase speak.
- **#192:** macOS-native STT + injected TTS smoke.
- **#170 Stages B, E and F:** the OpenRAIL-M disclosure ships before any default flip.
- **Whisper reuse ignored test (owner, from PR #262):** run `reused_stream_does_not_bleed_prior_utterance` in `primer-speech/tests/whisper_stream_reuse.rs` with real audio.
- **#135:** glib (blocked on Tauri 3).
- **Other:** QNN token stability, NPU tuning, latency-routing calibration.

## Open decisions / risks

### This session (#434, #435, #436)
- **#434: other folders' caches still accumulate,** one per folder ever used (a few MB each), by owner choice. Only the legacy `<model>.db` is deleted.
- **#434: a copied cache carries every locale of its source.** Each locale is topped up and pruned the first time it is opened, so another folder's DE passages sit in the copy until a DE session starts.
- **#434: "newest" means the newest mtime.** That is usually the most recently embedded cache, but a cache that was only opened (nothing written) keeps its old mtime.
- **#434: an older pre-#401 binary** run on the same machine recreates `<model>.db` and pays one embed.
- **#435: llamacpp and qnn legs** read their GGUF path or bundle from shared params, so two such legs are the same model in practice. They are flagged only when their configured model strings also match.
- **#435: the notice is English-only and operator-facing,** like the others.
- **#436: no behaviour change was intended,** but there are many `cfg` arms. The Android cross-compile was left to CI.

### From #431 / #432 (merged)
- **#431 (owner call from #430): the truncated partial is still spoken**, now as its own phrase. It is already on screen; dropping it in voice mode would need the DM to mark which chunks are the cut-off attempt.
- **#431: any blank line in a reply now ends a phrase**, also in the middle of a sentence an LLM wraps across a blank line (rare). Nothing is lost, only a pause is added.
- **#431: the pushed text now ends in `"\n\n"`.** Piper / Supertonic / macOS / `StubTts` trim phrases before synthesis and Android gets `trim()`ed text, so no engine sees the newlines. A future backend that synthesises `push_text` input verbatim must trim too.
- **#432: the GUI toast says only "error".** The detail is in `gui.log` (`primer::speech` warn). The text session the voice build created stays in `state.session`, exactly as after a "bye" exit.
- **#432: an `Err` now emits `Exit` before the join resolves.** `stop_voice_mode` still returns the `Err` if the user presses End voice mode afterwards; the frontend already left voice mode on the exit event.
- **Neither PR has been heard on real audio** (item 2).

### From #424–#427 (merged)
- **None of #425–#427 has been heard on real audio.** All checks are mock/stub-driven (item 2).
- **#427: the hold is measured from the start of each LLM attempt** (just after STT finalize), not from the VAD `SpeechEnd`. With Whisper's finalize latency the effective silence before speech is a bit over 1.2 s.
- **#427: a Stop mid-reply lets the ringbuf play out** (stage 4 would fix it). Stop is now checked before every phrase push, so no *further* phrase is synthesised.
- **#427: `STREAM_TAIL_FLUSH_IDLE` is a heuristic.** A model that stalls >300 ms right after a `.` that isn't a sentence end ("3." before "14") gets a phrase break there; nothing is lost.
- **#427: a cancelled streamed turn fires `on_response_complete`** (its chunks are already in a bubble); a cancel before SPEAK does not. Deliberate, but not pinned by a test.
- **#427: a spoken-but-cut-off reply is not in the session (D4 A).** If the child replies to it, the LLM has no context for it. Revisit option B if device logs show it often.
- **#427: the GUI now gets `on_transcript_finalized` mid-generation**, and chunks live during SPEAK. Before, both came after completion. The GUI observer handles chunks incrementally, but this is not watched in the webview.
- **#427: spurious VAD events keep the VAD arm open** (behaviour change; see the #427 section of `docs/handoffs/2026-11-25T100104Z.md`).
- **#426: D3:** an emphasis pair spanning a sentence boundary speaks its `*`.
- **#424: a Stop pressed during STT finalize**, a few ms before the LLM call, is dropped as stale.

### From #420 / #421 (merged)
- **#420: a parenthesised aside at an edge is dropped** ("(I think) it is the moon" → "it is the moon"). Whisper rarely brackets real words, but the hedge is lost when it does.
- **#420: an annotation in the middle (`why [BLANK_AUDIO] is it`) is kept verbatim.**
- **#420: the `. ! ?` sentence split also splits "4.5"**, but neither half is a silence phrase, so it is kept.
- **#421: the abort only lands at an `.await`.** A loop wedged inside a blocking call (a whisper finalize, Android TTS `onDone`) is cancelled only when that call returns. The 5 s stop still returns on time.
- **#421: the Android abort path has not been seen on a device** (it is in #260's checklist).


### From #418 (merged)
- **A lone "thank you" / "thanks for watching" / "you" (EN) or "vielen dank" / "danke" / "danke schön" (DE) is dropped silently.** These are the commonest Whisper silence artefacts, but a child can really say the first ones. There is a `shortcut:` comment in `silence_phrases_for`, and the PR flags it for the owner. The principled upgrade is Whisper's per-segment `no_speech_prob`, which `TranscriptSegment` discards today.
- **The filter applies to every STT backend.** Android and macOS-native don't hallucinate the Whisper subtitle strings, but the phrase table still drops a genuine "thank you" there.
- **No Hindi table:** the default arm is English, so a Hindi session gets the English phrases.
- **Filed from the review:** #419, fixed by #420 (at the string level, not per segment).
- **Not done (#326's other suggestions):** Parlor's ~300 ms tail-silence padding before Whisper (it lowers how often the filter has to fire), and the `no_speech_prob` gate.
- **The verbose `[stt] empty transcript` eprintln also fires for dropped noise.** That is harmless; the `debug!` line says which case it was.

### From #415 / #416 (merged)
- **#415: one more vendor patch to carry.** Drop it when an upstream `whisper-cpp-plus-sys` release depends on bindgen ≥ 0.72.1 (0.1.5 still pins 0.69).
- **#415: the vendored crate still downloads whisper.cpp from GitHub at build time.** The runtime stays offline.
- **#416: CI has no `macos-native-26` arm;** it was verified locally only.
- **Local gotcha:** a stale CMake cache in `espeak-rs-sys` causes `'math.h' file not found`. Run `cargo clean -p espeak-rs-sys`.

### From #410 (merged)
- **A transient blip on a local primary now fails over at once** to the secondary (often cloud) instead of retrying locally. More turns can reach the cloud on a flaky local server. The `--fallback-backend` / router opt-in is the consent, but the owner should confirm the trade-off (it is flagged in the PR).
- **Under the router, the cap follows leg order, not leg identity.** In `CloudPreferred` mode, or for a high-score hybrid turn, the *cloud* leg gets one attempt and local keeps the full budget.
- **The pin is a request count, not wall-clock.** The ceiling is now one full budget: ~1.75 s of backoff, or up to 2 × 5 s with `Retry-After`.
- **QNN and llamacpp ignore the cap** (they have no retry loop). Only the three HTTP backends read it.
- **`GenerationParams` is `Serialize`/`Deserialize`:** the new field defaults to `None` when absent.

### From #409 (merged)
- **No behaviour change.** It is still device-unverified: the Genie FFI path only runs on Android.

### From #413 (merged; DeepSeek backend)
- **`deepseek` uses `OpenAiCompatBackend::deepseek`**, with thinking disabled. The key comes from `DEEPSEEK_API_KEY`; the GUI reads the env var only.

### From #407 (merged)
- **The CLI prints its three `Engagement classifier: …` / `Concept extractor: …` / `Comprehension classifier: …` lines after all three are built**, not one at a time. On a failure, the earlier lines no longer print; the error still names the subsystem.
- **`build_served_subsystems` returns `Result<_, String>`**, not `PrimerError`, so both binaries keep their exact error text.

### From #408 (merged)
- **`dorny/paths-filter@v3` is a third-party action pinned by tag.** It needs `pull-requests: read` on the `changes` job only.
- **The ignore list lives in two places:** the `changes` filter and the `push` `paths-ignore`. Keep them in sync.
- **`validate gradle wrapper` is gated too**, so a docs-only PR skips it.

### From #403 (merged)
- **The stub-model warn fires on a stub primary run with a `--*-model` override.** Before, that run was silent.
- **The warn text says "the subsystem backend is stub" / "the served backend is stub"** instead of naming `--<subsystem>-backend`.
- **#362's pin covers `build_classifier` only.** Extractor and comprehension were already pinned by their ids.

### From #405 (merged; #401)
- **Upgrade cost:** fixed by #434. The legacy `<model>.db` seeds the new hashed cache and is then deleted.
- **Orphans accumulate**, one per seed dir ever used (a few MB each). #434 deletes only the legacy `<model>.db` and makes a new folder's first run cheap (copy-seed).
- **A moved checkout or a renamed app bundle is a new folder**, so it costs one full re-embed.
- **No seed files found** falls back to `<model>.db`, the legacy name.
- **The CLI's KB caller has no test seam.** Only the GUI caller is pinned.

### From #402 (merged)
- **The CLI embed hint prints after seeding, not before.** On a cache that is already complete it is correctly silent.
- **The GUI "knowledge base ready" log target is now `primer::startup`** (was `primer-gui::startup`).
- **`open_prepared_kb` returns `PrimerError::Knowledge`**; the GUI converts it with `.map_err(|e| e.to_string())`.
- **`primer-engine` now pulls `primer-kb-load`**, and with it `clap` + `tracing-subscriber` as normal deps.

### From #399 / #400 (merged)
- **#399: folding is per lookup, not per list.** Two different words that fold to one key both count as stopwords. Today this is harmless (`würde`/`wurde`, `hätte`/`hatte` are both on the list or both filler). Re-check whenever a new umlaut word is added.
- **#399: `ß` is not folded to `ss`**, so `weiß` (the colour white) stays a content word. A typed `weiss` is not a stopword either, which is correct for the same reason.
- **#399: no NFC pass on the query.** Non-stopword NFD tokens already match, because the tokenizer folds diacritics.
- **#399: the lookup now always allocates a folded `String`** per token. That is a handful per query, so trivial.
- **#400: the cache mirrors whatever corpus discovery found.** Pointing `PRIMER_SEED_DIR` at a smaller corpus prunes the cache to it. The next start with the full corpus re-inserts and re-embeds (~1 s per passage on BGE-M3). Nothing is lost: the cache is derived.
- **#400: a duplicate id stored before #383** is pruned only once (`stored_passages_in` keys by id), so one copy would survive. Latent.

### From #397 (merged)
- **Every startup with a cache reads every passage's text.** This is linear in corpus size; trivial at 91/66.
- **Any whitespace change in a seed line's `text` counts as an edit**, and costs one re-embed.
- **`load_jsonl` into a user-managed KB rewrites edited rows.** Those rows stay BM25-only (#389) until `primer-kb-load --reembed`.

### From #396 (merged)
- **Manual first-run GUI check not run** (item 4 above). Progress is one event per 16-passage batch (~18 s apart on BGE-M3); the restart button's long text may wrap on a narrow modal; voice-mode rebuilds report nothing; the CLI prints only its one-line notice.

### From #394 (merged)
- **First-run cost is large:** ~106 s for BGE-M3 over 91 EN passages on an M-series Mac. The first `--language de` run pays again for its 66 passages. Weaker hardware will be slower. Android stays `--embedder-backend none` (#157), so no cache is built there.
- **The cache is per machine, per model and per seed-corpus folder (#401), and holds no child data.** It is created with `create_private_dir_all` anyway. Deleting `~/.primer/kb/` is safe: it rebuilds on the next open.
- **An explicit `--knowledge-db` that was never re-embedded is still BM25-only with `min_score = 0.0` on the hybrid path.** That is the (b) half of #388, not done. Only the stopword filter (#386) defends it.
- **An embed failure at startup is a `warn!` only** (CLI stderr and GUI `gui.log`). The session runs BM25 plus #389 hits and retries on the next open.
- **Model-slug collisions:** `slug()` lowercases, so model ids "BGE-M3" and "bge-m3" share a file. The second model then finds no missing passages, and its vector leg is empty (a `warn!`, BM25 fallback). This is rare: both names would have to be configured on one machine.
- **#389 hits use `KB_BM25_ONLY_MIN_SCORE`, not `HybridParams.min_score`** (that one is an RRF floor and meaningless for BM25 scores).
- **The GUI now builds the embedder before opening the stores.** A locale-mismatch error therefore surfaces only after the (seconds-long) fastembed load.

### Carried from earlier sessions
- **#390 (stopwords):**
  - **The lists are hand-built.** Every added word must not appear in a real child question. Re-run `retrieval_quality{,_de}` + `off_topic_bm25` + `bm25_floor_tripwire`.
  - **BM25 margin narrowed:** min top-K 0.80 and worst top-1 1.12, against the 0.5 floor.
  - **Hindi has no list.**
- **#387 (off-topic gate):**
  - **Recall margin is thin:** 0.023 below the lowest benchmark query ("was ist schall", 0.473). An on-topic question just under 0.45 gets **no** grounding (now possibly some, via #389, on a partly embedded KB).
  - **Only `bge-m3` is gated**, and the gate depends on the model id string.
  - **`HybridParams` is shared with LTM**, which ignores `min_top_cosine`.
- **#385:** a resume right after a turn can block for up to the background budget (10 s worst case).
- **#384:** `finish()` only recovers a complete-but-unterminated final unit.
- **#381:** a `force` reembed clears the locale's vectors up front. A model still used by another locale stays pinned at its old dim.
- **#382:** a *write* failure in a later seed file still leaves earlier files committed (`shortcut:` in `load_seed_files`).
- **#374:** a corrupt `embedding_models` row is a hard retrieval error (intended).
- **#369 residual:** a session-build failure after the close still loses the text session (`shortcut:` in `start.rs`).
- **#370:** v9 drop/rename runs with `foreign_keys = ON`.
- **Mislabelled rows from before #363** keep the wrong id (#352 item 4).
- **#360 permissions:** existing `0644` DB files are protected only by the `0700` parent; Windows and Android are untouched.
- **GUI logging:** rotation only at startup; the log may hold child text.
- **Notice text is English-only** and operator-facing.
- **Stale CMake cache after Xcode/SDK upgrades:**
  1. `find src/target -name CMakeCache.txt -maxdepth 6 | xargs grep -l 'MacOSX<old>'`
  2. `cargo clean -p <those sys crates>`
- **The `github` MCP server fails to connect** (bad Authorization header). The `gh` CLI works.
- **zsh gotchas:**
  - `$var` is not word-split; use `${=spec}`.
  - **Quote `echo '==='`**; an unquoted `===` errors with "== not found".
  - Use `git grep … -- '*.rs'`.
- **Cargo stops at the first failing test crate.** `primer-knowledge` has no `fastembed` feature; use `-p primer-kb-load --features fastembed` for the fastembed arm.
- **A foreground `cargo test --workspace` exceeds the 600 s tool timeout** when something slow sneaks in. Run it with `run_in_background` and grep the log.


## Patterns to reuse, not reinvent

- **Copying a SQLite file safely (new, #434):** use `VACUUM INTO` on a `SQLITE_OPEN_READ_ONLY` connection (`primer_knowledge::snapshot_db`), not `fs::copy`, which can tear while another process writes. Write into a per-process temp file, then `std::fs::hard_link` it into place: unlike `rename`, a hard link fails if the target exists, so a concurrent winner's open file is never replaced.
- **Splitting a long `async fn` (new, #436):** move each block into a `fn(cli: &Cli) -> Struct`, where the struct's fields have the same names as the old locals, and destructure at the call site (`let Inference { backend, notice, .. } = build_inference(&cli).await;`). The rest of the body needs no edit. Watch for moves out of the owned struct (`cli.session_db` → `.clone()`) and `&cli` → `cli`.
- **A notice for a misconfiguration that still works (new, #435):** keep the rule a pure `fn(..) -> bool` tested on its own, and build the notice only in the `Wrapped` arm, so a real degradation (`FallbackDropped` / `PrimaryUnavailable`) still wins.

- **Reporting a generic loop's `?` exits once (new, #432):** instead of adding a report before every `?`, rename the body (`run_loop_body`), give it `&mut O`, and have the old entry point call it and report on `Err`. Inside the body, pass `observer` (not `&mut observer`) to helpers taking `&mut O`: it reborrows implicitly.
- **A hard boundary that survives a second splitter (new, #431):** when text passes through two splitters (the voice loop's, then each TTS backend's), make the first one's output end in a token the second treats as a hard boundary (`PHRASE_BREAK`), so an unterminated phrase is never re-glued. Test it through `StubTts` (it has its own splitter); a recording mock without one cannot catch it.
- **Paused-clock async tests (new, #427):** `#[tokio::test(start_paused = true)]` (needs tokio `test-util`; `primer-speech` has it as a dev-dependency). A `tokio::time::sleep` in a mock responder, or a `sleep_until` hold in the code under test, auto-advances as soon as every task is idle, so a 1.2 s hold or a 10 s "still generating" sleep costs nothing. Record `tokio::time::Instant::now()` in a mock to assert *when* something happened. Wake the test with a `tokio::sync::Notify` from the mock instead of polling.
- **Stacked PRs (new):** when several PRs edit one file, branch each from the previous one and open it with `gh pr create --base <previous-branch>`. Say "merge in order" in each body. GitHub retargets the next PR to `main` when its base merges (with the squash merges used here, check that the retargeted diff is still clean).
- **One `loop { select! }` instead of nested selects (new, #427):** the old LATENT_THINK had three near-copies of a select for "events open", "events closed", and "spurious event". A loop with `if <cond>` preconditions on arms (`events.recv(), if events_open`, `sleep_until(..), if !ready.is_empty()`) and `Some(d) = rx.recv()` patterns covers every case in one place, and `break <outcome>` gives the loop a value.
- **A shared flag set per call but cleared per call (new, #426):** a guard that clears a shared "busy" flag at the end of *each* call leaves a gap between calls that another task can observe. When the caller makes several calls per logical operation, own the flag for the operation (set on first call, clear in `Drop`). See `AndroidTtsSession`.

- **Vendoring a `-sys` crate for a one-line dep bump (new, #415):**
  1. `cp -R ~/.cargo/registry/src/*/<crate>-<ver> src/vendor/<crate>`.
  2. Edit the normalised `Cargo.toml` (the one cargo reads) and `Cargo.toml.orig`.
  3. Add a `[patch.crates-io]` entry with a rationale comment.
  4. **Do not run `cargo update -p`**: it re-resolved unrelated windows-sys versions. Run `git checkout Cargo.lock`, then `cargo metadata`, to get the minimal lock diff.
- **Proving a CI fix really ran (new):** `gh run view --job <id> --log | grep -E 'Image:|Compiling <crate>'`. It confirms both the runner image and that the crate was built fresh rather than restored from cache.

- **Per-call knobs a decorator sets on its legs (new):** add an `Option` field to `GenerationParams` (`#[serde(default)]`) and a method that returns a modified copy (`for_failover_leg`). Don't rebuild backends with different settings: the router's leg order changes per turn. Each decorator mock records `params.<field>` in a `Mutex<Vec<_>>`; clone the `Arc` before handing it to the decorator, so the test can still read it.
- **Counting retries over real HTTP (new):** `test_http::serve_503_counting()` answers every request with 503 and counts them, and `requests_until_pre_stream_error(|url| Backend::new(url, …), &params)` wraps it. Assert on request counts, not elapsed time.
- **Local Android cross-clippy (new):** `ring`'s build script needs the NDK C compiler. Set `CC_aarch64_linux_android`, `AR_aarch64_linux_android` and `CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER` from `~/Library/Android/sdk/ndk/29.*/toolchains/llvm/prebuilt/darwin-x86_64/bin` (command in the resume block).
- **One helper for a sequence both binaries repeat (new):** when the CLI's monolithic `async_main` copies a GUI construction step, move the step into `primer-engine` and test it there (#402's `open_prepared_kb`, #407's `build_served_subsystems`). The CLI then has nothing left to get wrong.
- **CI gates that skip a required job must fail open (new):** a job skipped because a `needs:` job failed counts as passing. Use `!cancelled() && (needs.X.result != 'success' || …)` so a broken filter runs everything.
- **Proving an `Arc` is shared, not rebuilt (new):** when an id can't tell a clone from a fresh build, hand in a test double whose output no real backend could produce (canned engagement JSON), and assert the consumer returns it. See `CannedAssessmentBackend` in `primer-engine/src/wiring/classifier_construction_tests.rs`.
- **Warn-only branches (new):** pull the condition into a pure `fn … -> Option<&'static str>` reason and test that, instead of capturing `tracing` output. See #403's `stub_ignores_model`.
- **Startup hints from progress callbacks (new):** instead of pre-checking state to guess whether a slow step will run, print from the step's own progress callback on its first `(0, total > 0)` call. #402's CLI embed hint was the example.
- **Python edit guards: assert per replacement, not a total count (new):** a file-wide `s.count(', None)')` also matched an unrelated earlier line and aborted the write. Use a `sub(a, b)` helper that asserts `s.count(a) == 1` for each edit.
- **Diacritic-insensitive matching (new):** `unicode_normalization::{UnicodeNormalization, char::is_combining_mark}`: `s.nfd().filter(|c| !is_combining_mark(*c)).flat_map(char::to_lowercase)` reproduces FTS5 `unicode61` folding. It does not fold `ß`.
- **Syncing derived rows against a source of truth (new):** before adding a hash column + schema bump, check whether the stored row already holds what you would hash. #392 compared stored `(source, text)` directly. Deleting from an external-content FTS5 table needs the old values: `INSERT INTO t(t, rowid, …) VALUES('delete', rowid, …)` (`LocaleSql::delete_fts`).
- **Splitting a lib.rs by byte offsets (new):** slice the file at two unique doc-comment anchors in Python, write the pieces to new files with their own `use` lines, add `mod x; pub use x::*;`, then `cargo build` to find the stale imports. Compare test counts with `git stash -u` / `stash pop`.
- **Progress from a long Rust step to the webview (new):** pass `&mut (dyn FnMut(..) + Send)` down the call chain (it keeps the Tauri command future `Send`). The command closes over `&AppHandle<R>` and calls `app.emit(CONST, payload)`. Test it with `tauri::test::mock_app()` + `app.listen(CONST, ..)`, then call the generic command with `app.handle().clone(), app.state()`. Rust listeners fire synchronously, so assert straight after the `await`.
- **Smoke-running the real CLI without touching the owner's home (new):** make a scratch `HOME`, symlink `~/Library/Caches/primer` into `$HOME/Library/Caches/primer` (so BGE-M3 isn't re-downloaded: `dirs::cache_dir` follows `$HOME`), and set `PRIMER_SEED_DIR=/Users/hherb/src/primer/data/seed`. A directly-run binary has no `CARGO_MANIFEST_DIR`, so without it seed discovery finds nothing. Grep `RUST_LOG=primer=info,primer_kb_load=info,primer::retrieval=debug` for `ready` / `reembedding` / `gate`.
- **GUI tests must not use the default embedder (new):** `GuiConfig::default()` picks fastembed on a default build, and since #394 a session build then embeds the whole corpus (~100 s per test). Set `cfg.embedder.kind = "none"` in any test config that builds a session, or use a `StubEmbedder` when the KB is under test (`stage_tests.rs::knowledge_base_with_embedder_*`). A `primer-gui` test run over ~10 s is the symptom.
- **Old-behaviour baseline for the stopword filter (new):** open a KB under `Locale::Hindi` (empty list) and `load_jsonl` the EN seed files into it. That gives the pre-#386 ranking side by side with the filtered one in a single probe run.
- **Before/after sweep numbers (new):** `git stash -u` → run the `#[ignore]`'d sweep → `git stash pop`. To see which `KNOWN_FAILING_*` entries now pass, empty the list temporarily (perl), run `retrieval_quality`, then restore from a scratchpad backup.
- **Per-token blame for a leaky query (new):** run each token alone through `kb.retrieve` at the production params. #386's probe showed the very common words ("is", "the", "can") were already under the 0.5 floor, and the leaks came from mid-frequency function words plus content words.
- **Calibrating a retrieval threshold (new):** measure the distribution *before* designing the fix. A throwaway `tests/zz_probe.rs` that embeds every seed passage (read the JSONL, skip `#` comment lines) and prints per-query best cosine + BM25 hit counts answered #307 in one run. The committed version is `off_topic_gate.rs::off_topic_gate_probe`. When the distributions overlap, report the numbers and let the owner pick the trade-off.
- **Struct-literal field additions across the workspace (new):** `git grep -n "HybridParams {"` to list them. A regex keyed on `min_score: …,\n source_filter` also hits `RetrievalParams` literals, so check each match.
- **Driving a real streaming backend end to end (new):** `crate::test_http::serve_once(body)` in `primer-inference` answers one request with `200` + `body` and then closes, with no `Content-Length`. Point `OllamaBackend::new` / `OpenAiCompatBackend::new` / `CloudBackend::new` at the returned URL, then `crate::test_http::collect(stream)`. Use it for any spawn-loop behaviour (EOF, mid-stream error payloads) that buffer-level tests can't reach.
- **Restructuring a big match arm without retyping it (new):** a Python regex that captures the existing blocks and reassembles them, then `cargo fmt`, then verify with `git diff -w` that only the intended lines changed. Assert exactly one match before writing.
- **JS behaviour without a JS runner:** use an `include_str!` contract test in `primer-gui/src/*_contract.rs`. Slice the function body by a unique anchor string and assert the call is there. See `voice_stop_contract.rs` and `diagnostics_toggle_contract.rs`. Mutation-check by stashing the `ui/` changes and re-running.
- **Cross-file JS hooks:** app.js exports `window.primer*` functions at the bottom; voice.js lists the ones it uses in its header comment. Add new hooks in both places.
- **Migration tests:** pin *behaviour* (cascade fires, UNIQUE rejects), not DDL text. Build `fresh_vN_conn()` by chaining the earlier helpers. Fixtures should call the real `apply_vN_migrations`; never hand-roll the DDL.
- **Effective backend:** use `primer_engine::served_identity(...)` everywhere. Never re-derive it from `config.backend.kind`.
- **End-to-end degraded-primary GUI tests:** pair a `qnn` primary with a `cloud` fallback and an Inline key (`primary_unavailable_cloud_fallback_config()`).
- **Misbehaving-embedder tests:** wrap `StubEmbedder` and alter its output (`DropsLastEmbedder` in `primer-kb-load/src/tests.rs`). `Named` lives at `primer_core::speech::Named`.
- **Corrupt-row DB tests:** register a row, then `UPDATE … SET <int col> = 'not-a-number'` to force a read-side type error (`conn_with_corrupt_dim`).
- **Forcing a mid-transaction write failure:** open a second `Connection` on the same temp file and add `CREATE TRIGGER … BEFORE INSERT … WHEN NEW.id = 'bad' BEGIN SELECT RAISE(ABORT, 'x'); END`, or `DROP` the FTS table. See `insert_passages_rolls_back_*` in `primer-knowledge/src/tests.rs`.
- **Mutation-check a transaction:** remove the transaction entirely. Writing through `&conn` while a `BEGIN` is still open on the same connection is NOT a valid mutant: those writes are still inside the transaction.
- **DM background-task tests:** `await_pending_background()` is `pub(super)` and reachable from `dialogue_manager/tests/*`. Call it directly to force "what the next turn would apply" without scripting a second turn.
- **Env-var-dependent code:** inject `impl Fn(&str) -> Option<String>`; never use `set_var`. For seed discovery, test `load_seed_files(kb, &paths)` directly instead of setting `PRIMER_SEED_DIR`.
- **Mutation-check new tests:**
  1. Back up the file to the scratchpad.
  2. Apply the mutation with `perl -0pi`.
  3. Run the filtered tests.
  4. Restore the file.
- **Python `sub()` edits:** a helper that asserts the match count stops before the write.
- **Bug-fix recipe:**
  1. Branch off `origin/main`, and check `git branch --show-current` **before** editing. An earlier session started #314 on the previous fix's branch, and had to `git stash` → `checkout main` → `checkout -b` → `stash pop` before committing.
  2. Failing test, then fix, then green.
  3. Test + clippy each touched crate, plus its feature arms. Then fmt and rustdoc.
  4. `git grep` the docs for drift.
  5. Commit with `Closes #N`, push, open the PR.
  6. File any out-of-scope half as an issue.
- **Split recipe, substantive-line diff, Dependabot triage:** see `docs/handoffs/2026-10-16T202752Z.md`.
- **Driving a real `#[tauri::command]` in a test:** make it generic over `R: tauri::Runtime`, then call it with `tauri::test::mock_app()` + `app.manage(AppState::new(..))` + `app.state()`. See `commands/voice/tests.rs::app_with_text_session`. A command taking only `tauri::State` needs no generic.
- **Migration that changes an existing FK:** follow v9 (`pragma_foreign_key_list` guard + a create / copy / drop / rename tx).
- **Session start:** check `git status` first, and read `NEXT_SESSION.md` AFTER `git pull`.


## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status --short && git checkout main && git pull --ff-only && git log --oneline -8 && gh pr list --state open
# Open at close: #434 (#406), #435 (#352 item 2), #436 (CLI split), docs PR — independent. After merge: gh issue view 406 --json state
S=/private/tmp/primer-scratch; mkdir -p "$S"   # or the session's scratchpad dir

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done

# === #434 / #435 / #436 regression checks ===
~/.cargo/bin/cargo test -p primer-engine --lib -- knowledge:: build_main_backend
~/.cargo/bin/cargo test -p primer-knowledge --lib snapshot_db
for f in "" speech qnn llamacpp "speech,macos-native" "speech,macos-native-26"; do echo "== $f"; ~/.cargo/bin/cargo clippy -p primer-cli ${f:+--features $f} --all-targets -- -D warnings 2>&1 | grep -E '^error'; done

# === #424-#427, #431, #432: voice loop on every speech arm (stream_tests needs tokio test-util, already a dev-dep) ===
for f in voice-loop voice-loop,silero,cpal,macos-native voice-loop,silero,cpal,macos-native-26 android-native,voice-loop voice-loop,silero,whisper,piper,cpal,supertonic; do \
  echo "== $f"; ~/.cargo/bin/cargo test -p primer-speech --features $f 2>&1 | grep -E 'FAILED|panicked|^error'; done
~/.cargo/bin/cargo test -p primer-speech --features voice-loop --lib -- stream_tests stale_cancel cancel_during speak_pushes tts_phrases phrase_split tts_error
~/.cargo/bin/cargo test -p primer-speech --features android-native --lib -- android::tts
# === #421: voice stop abort (speech + android-native arms) ===
~/.cargo/bin/cargo test -p primer-gui --features speech --lib -- join_or_abort aborting_a_wrapper
~/.cargo/bin/cargo test -p primer-gui --features android-native --lib -- join_or_abort aborting_a_wrapper
# === #415: whisper path with the vendored bindgen 0.72.1 ===
~/.cargo/bin/cargo clippy -p primer-cli --features speech --all-targets -- -D warnings
# === #410 regression checks ===
~/.cargo/bin/cargo test -p primer-inference --lib -- fallback router failover_leg_cap
~/.cargo/bin/cargo test -p primer-core --lib -- retry gen_params
# === #409: qnn arm (host + android; NDK env for ring) ===
~/.cargo/bin/cargo test -p primer-inference --features qnn 2>&1 | grep '^test result'
B=~/Library/Android/sdk/ndk/29.0.14206865/toolchains/llvm/prebuilt/darwin-x86_64/bin; CC_aarch64_linux_android=$B/aarch64-linux-android24-clang AR_aarch64_linux_android=$B/llvm-ar CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$B/aarch64-linux-android24-clang ~/.cargo/bin/cargo clippy -p primer-inference --features qnn --target aarch64-linux-android --all-targets -- -D warnings

actionlint .github/workflows/ci.yml   # after any workflow edit

# === Pick the next bug ===
gh issue list --state open --limit 40

# === Standard workspace gate (one cargo pass at a time; run the test pass in the background) ===
~/.cargo/bin/cargo test --workspace > $S/ws.log 2>&1; grep -E 'test result: FAILED|^error|over 60' $S/ws.log  # expect empty
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo clippy -p primer-gui --features speech --all-targets -- -D warnings
~/.cargo/bin/cargo clippy -p primer-gui --features android-native --all-targets -- -D warnings
~/.cargo/bin/cargo clippy -p primer-kb-load --features fastembed --all-targets -- -D warnings
~/.cargo/bin/cargo fmt --all -- --check
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test off_topic_gate --test retrieval_quality_hybrid --test retrieval_quality_hybrid_de

# === Rustdoc gate (CI-enforced); feature arms mirror ci.yml ===
RUSTDOCFLAGS="-D warnings" ~/.cargo/bin/cargo doc --workspace --no-deps --document-private-items
for spec in "-p primer-speech --features voice-loop,silero,whisper,piper,cpal,supertonic" "-p primer-speech --features android-native" "-p primer-speech --features voice-loop,android-native" \
  "-p primer-inference --features qnn" "-p primer-inference --features llamacpp" "-p primer-gui --features speech" "-p primer-gui --features android-native" \
  "-p primer-cli --features speech" "-p primer-speech --features voice-loop,silero,cpal,macos-native" "-p primer-speech --features voice-loop,silero,cpal,macos-native-26"; do \
  RUSTDOCFLAGS="-D warnings" ~/.cargo/bin/cargo doc ${=spec} --no-deps --document-private-items >/dev/null 2>&1; echo "$spec exit=$?"; done
```

## Reporting back

- **#434:** a new copy of the project, or the first start after an upgrade, used to spend about a minute and a half rebuilding the knowledge library from scratch, and left the old copy on disk forever. It now starts from the library already on the machine, rebuilds only what changed, and deletes the old copy. Closes #406.
- **#435:** setting the backup AI to the very same server and model as the main one quietly gave no protection. The app now says so in its warning banner. Part of #352.
- **#436:** the command-line program's 1,362-line main file is now five smaller files, with nothing changed for users.
