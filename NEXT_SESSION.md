# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-11-23 (UTC).

**Three PRs are open against `main`. They touch different files and merge in any order:**
- **#415** (`fix/whisper-bindgen-414`): closes #414. CI is green.
- **#416** (`refactor/split-macos-tts`): splits `macos/tts.rs`. CI is green.
- **The docs-only handoff PR** (`docs/next-session-2026-11-23`) carries this brief.

- **`main` is at `6f0d72d`** (#413, the DeepSeek backend).

**Since the last brief:**
- #409, #410, #411 and #413 merged. #312 is CLOSED.
- **#404 proof passed:** on #411, `detect code changes` passed, every heavy job was skipped, and the PR merged.
- **New #414:** GitHub moved `ubuntu-latest` to ubuntu-26.04 (libclang 22). #413 had pinned LLVM 21 in CI as a stopgap.

## What we shipped this session

### #415 — `b83bea4`: closes #414 (the Linux speech build breaks under libclang 22)
- **Cause:** `whisper-cpp-plus-sys` 0.1.4, and the newer 0.1.5, pin `bindgen 0.69`. Under clang 22 that emits `whisper_full_params` as an empty struct (rust-bindgen#3264, fixed in 0.72.1), so `whisper-cpp-plus` fails with 40× E0609.
- **Fix:** the crate is vendored verbatim at `src/vendor/whisper-cpp-plus-sys/` (~100 KB; it downloads whisper.cpp at build time). The only change is `bindgen = "0.72.1"`. It is wired via `[patch.crates-io]`.
- The CI `LIBCLANG_PATH=/usr/lib/llvm-21/lib` pin is dropped.
- The `Cargo.lock` diff is minimal: bindgen 0.69.5 → 0.72.1 for this crate only.
- There is a CLAUDE.md vendor bullet.
- **Proof:** #415's `cargo check (non-default features)` ran on the `ubuntu-26.04` image with no pin. It compiled the vendored crate fresh, regenerated the bindings, and checked `whisper-cpp-plus` cleanly.
- **On macOS:** clippy `-D warnings` is clean for `primer-speech --features whisper` and `primer-cli --features speech`. `primer-speech --features whisper` tests pass.

### #416 — `6ad4833`: split `primer-speech/src/macos/tts.rs` (668 lines; behaviour-preserving)
- **`tts/mod.rs` (329):** the struct, its trait impls, `MacosTtsSession` and the sample-rate query.
- **`tts/synthesis.rs` (353):** `synthesize_streaming` (now `pub(super)`), the main-thread and background paths, `stream_pcm_callback`, and the drain consts.
- **Verification:**
  - In Python, the file was sliced at unique anchors, with an assert that the pieces rebuild the original.
  - The normalised code diff shows only rewrapped `use` lists.
  - Test counts are unchanged: 132 (`macos-native`) and 168 (`macos-native-26`). Both arms pass, including the real-AVSpeech `macos_tts` binary.
  - clippy and rustdoc are clean on both arms.
- CLAUDE.md and two path comments are updated.

> SHAs are branch-tip commits as pushed; squash-merge gives new ones on `main`.

### Per-session sweep
- **Inline-test detector:** clean.
- **Oversized list:** production files over 500 lines, after #416:
  - `primer-cli/src/main.rs`: 1362 (grew by 13 with #413);
  - `state_machine/inner.rs`: 506;
  - `primer-speech/src/macos/stt.rs`: 504;
  - `primer-inference/src/cloud.rs`: 503 (new to the list).

## What's next (concrete acceptance criteria)

### 0. Owner review/merge: #415, #416, and the handoff PR
- **Acceptance:** CI green and all three merged. Then `gh issue view 414 --json state` shows CLOSED.
- If #416's `cargo clippy (macOS feature combos)` fails, check the `macos-native` arm first. The other arms don't compile `macos/tts/`.

### 1. Sweep + inline-test detector: re-run EVERY session
Commands are in the resume block.

### 2. Manual first-run GUI check for #396 (owner)
Still not watched in the real webview: `mv ~/.primer/kb ~/.primer/kb.bak`, launch the GUI, Start new → Save & start; the button should count "… 0 / 91" upward.

### 3. Other bugs (host-completable; TDD each)
- **#406:** stale KB cache files are never cleaned up. Decide a pruning policy first (owner).
- **#371:** needs a `JoinHandle::abort` design note first.
- **#352 remaining:** item 2 needs the owner decision below; items 1 and 4 need a design note first.
- **Older:** #315, #316, #325/#326. Run `gh issue list` to pick; read each before starting.

### 4. Production-code splits (owner-approved lane, all feature-gated, so dual-verify)
- **`primer-cli/src/main.rs` (1362)** is the only real offender left, and the hardest. Its `async_main` is monolithic.
- Prefer the "one helper both binaries repeat → `primer-engine`" pattern over a mechanical slice. That moves logic *and* gives it a test seam.
- `inner.rs` (506), `stt.rs` (504) and `cloud.rs` (503) are only marginally over the limit. Split them only if a change grows them.
- **Recipe that worked for #409 / #416:**
  1. In Python, slice the file at unique anchors (section banners are best; check that a doc-line anchor isn't repeated further down).
  2. Assert that the pieces rebuild the whole file, and write the new files.
  3. `cargo clippy` lists the stale imports.
  4. Compare the code lines with a normalised diff (drop comments and `use` lines, sort).
  5. Compare test counts with `--list | grep -c ': test$'` before and after.

### Carried / owner-or-hardware-gated
- **#260:** Android-voice on-device acceptance; also check that a denied mic keeps the text session (#369).
- **#192:** macOS-native STT + injected TTS smoke.
- **#170 Stages B, E and F:** the OpenRAIL-M disclosure ships before any default flip.
- **#166 item #1:** WhisperStream reuse smoke.
- **#135:** glib (blocked on Tauri 3).
- **Other:** QNN token stability, NPU tuning, latency-routing calibration.

## Open decisions / risks

### This session (#415)
- **One more vendor patch to carry.** Drop it when an upstream `whisper-cpp-plus-sys` release depends on bindgen ≥ 0.72.1 (0.1.5 still pins 0.69). Check with `curl -s https://crates.io/api/v1/crates/whisper-cpp-plus-sys/<ver>/dependencies`.
- **The vendored crate still downloads whisper.cpp from GitHub at build time.** It did before too; that is build-time only, and the runtime stays offline.
- **Users on Linux with libclang 22+ are fixed only once #415 merges.** Before that, `LIBCLANG_PATH=/usr/lib/llvm-21/lib` is the workaround.
- **Not checked:** whether any other bindgen-0.69 consumer (`bindgen 0.69.5` remains in the lock) breaks under clang 22. CI's speech and default arms passed on ubuntu-26.04, so none of the built ones do.

### This session (#416)
- **No behaviour change.** CI has no `macos-native-26` arm; it was verified locally only.
- **Local gotcha:** this session hit the stale-CMake-cache failure in `espeak-rs-sys` (`'math.h' file not found`) on the `--features …macos-native` test build. `cargo clean -p espeak-rs-sys` fixed it.

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
- **Upgrade cost:** existing `~/.primer/kb/<model>.db` caches are orphaned (safe to delete). The first start after upgrading re-embeds into the new file: ~100 s on BGE-M3 for EN, again for DE.
- **Orphans accumulate**, one per seed dir ever used (a few MB each). Nothing cleans them.
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
- **Manual first-run GUI check not run** (item 2 above). Progress is one event per 16-passage batch (~18 s apart on BGE-M3); the restart button's long text may wrap on a narrow modal; voice-mode rebuilds report nothing; the CLI prints only its one-line notice.

### #352 item 2: what counts as a pointless fallback? (owner decision, unchanged)
`--fallback-backend == --backend` with a different **model** is legitimate (small/big ollama models, or hybrid routing on one server).
- (a) notice only when kind **and** model match;
- (b) kind-only, worded as "no protection if the server goes down";
- (c) drop the item and keep the `tracing::warn!`.

**Recommendation:** (a), and downgrade or drop today's kind-only warn on the router path.

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
  1. Branch off `origin/main`, and check `git branch --show-current` **before** editing. This session again started #314 on the previous fix's branch, and had to `git stash` → `checkout main` → `checkout -b` → `stash pop` before committing.
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
cd /Users/hherb/src/primer && git status --short && git checkout main && git pull --ff-only && git log --oneline -6 && gh pr list --state open
# Open at close: #415, #416, the handoff PR. After merge: gh issue view 414 --json state
S=/private/tmp/primer-scratch; mkdir -p "$S"   # or the session's scratchpad dir

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done

# === #416: both macOS arms (test count 132 / 168) ===
for f in macos-native macos-native-26; do ~/.cargo/bin/cargo test -p primer-speech --features voice-loop,silero,cpal,$f -- --list 2>/dev/null | grep -c ': test$'; done
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

- **#415:** Linux machines with the newest compiler tools (Ubuntu 26.04, which GitHub now uses for CI) could no longer build the Primer's voice mode. A third-party speech component used an outdated code generator. We now ship our own copy of that component with the generator updated, so voice mode builds there again with no workaround. Closes #414.
- **#416:** a housekeeping change. The Mac's built-in text-to-speech code is now in two smaller files instead of one large one. Nothing behaves differently.
