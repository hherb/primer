# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-11-07 (UTC).

**Three PRs are open against `main`. They touch different files and merge in any order:**
- **#409** (`refactor/split-qnn-genie-real`): splits `qnn/genie/real.rs`.
- **#410** (`fix/failover-retry-budget-312`): closes #312.
- **The docs-only handoff PR** (`docs/next-session-2026-11-07`) carries this brief and is the #404 proof (item 0).

- **`main` is at `f8afd12`** (#408).

**Since the last brief:** #407 and #408 merged; #364 and #404 are CLOSED.

## What we shipped this session

### #409 — `ca2d6d5`: split `primer-inference/src/qnn/genie/real.rs` (566 lines, behaviour-preserving)
- It is now `real/mod.rs` (188 lines: library, `open_dialog`, config guard), `real/logger.rs` (167: optional `PRIMER_GENIE_LOG_PATH` logging and the log guard), and `real/dialog.rs` (244: the owning dialog and the C callback).
- Only `pub(super)` and module paths changed. The normalised code diff shows only imports and the `mod` lines.
- Three rustdoc links to `RealGenieLibrary::open_dialog` now point at `GenieLibrary::open_dialog`. It is a trait method, so the old link does not resolve without the trait in scope.
- `primer-inference --features qnn`: 292/0 before and after. clippy is clean on host and on `aarch64-linux-android`. qnn rustdoc is clean.
- There is a one-line CLAUDE.md note in the QNN bullet.

### #410 — `be17124`: closes #312 (retry budgets stacked under `FallbackBackend` / `RouterBackend`)
- New `GenerationParams.max_retry_attempts: Option<u32>` (`#[serde(default)]`).
- New `GenerationParams::for_failover_leg()`, which caps it at the new `consts::retry::FAILOVER_LEG_MAX_ATTEMPTS = 1`.
- New `RetrySettings::capped(cap)`, which lowers `max_attempts` and never raises it. The cloud, Ollama and openai-compat backends apply it.
- `FallbackBackend` caps the primary. `RouterBackend` caps the first leg when a second leg exists. The last leg keeps the full budget. Two down legs now cost 1 request plus one full budget, instead of two full budgets.
- **Tests (all mutation-checked):**
  - pure `capped` and `for_failover_leg`;
  - the decorator mocks record each leg's cap (both router orders, plus `LocalOnly`);
  - each HTTP backend makes 1 request against an always-503 server (new `test_http::serve_503_counting` / `requests_until_pre_stream_error`);
  - the two-leg worst case end to end: `fallback::tests::two_down_legs_spend_one_plus_one_full_budget`.
- There is a CLAUDE.md note in the retry bullet.

### Gate (#410, which includes everything)
- `cargo test --workspace --no-fail-fast`: 1525/0.
- clippy `-D warnings`: workspace `--all-targets`, `primer-gui --features speech` / `android-native`, `primer-cli --features qnn` / `llamacpp`.
- fmt and workspace rustdoc.
- **Not run:** the rustdoc feature-arm loop (except the qnn arm for #409) and the fastembed recall tests. No retrieval code changed.

> SHAs are branch-tip commits as pushed; squash-merge gives new ones on `main`.

### Per-session sweep
- **Inline-test detector:** clean.
- **Oversized list:** production files over 500 lines, after #409: `primer-cli/src/main.rs` 1349, `primer-speech/src/macos/tts.rs` 668, `state_machine/inner.rs` 506, `primer-speech/src/macos/stt.rs` 504.

## What's next (concrete acceptance criteria)

### 0. Owner review/merge: #409, #410, and the handoff PR (#404 proof)
- **Acceptance:** CI green and all three merged. Then `gh issue view 312 --json state` shows CLOSED.
- **#404 proof:** the handoff PR touches only `NEXT_SESSION.md` and `docs/handoffs/`. It must show `detect code changes` passing, every heavy job **skipped**, and it must merge without a bypass. If the heavy jobs run, or the PR is blocked, reopen #404.

### 1. Sweep + inline-test detector: re-run EVERY session
Commands are in the resume block.

### 2. Manual first-run GUI check for #396 (owner)
Still not watched in the real webview: `mv ~/.primer/kb ~/.primer/kb.bak`, launch the GUI, Start new → Save & start; the button should count "… 0 / 91" upward.

### 3. Other bugs (host-completable; TDD each)
- **#406 (new from #405):** stale KB cache files are never cleaned up. Decide a pruning policy first.
- **#371:** needs a `JoinHandle::abort` design note first.
- **#352 remaining:** item 2 needs the owner decision below; items 1 and 4 need a design note first.
- **Older:** #315, #316, #325/#326. Run `gh issue list` to pick; read each before starting.

### 4. Production-code splits (owner-approved lane, all feature-gated, so dual-verify)
- **`primer-speech/src/macos/tts.rs`** (668, `macos-native`) is next; then `stt.rs` (504), `state_machine/inner.rs` (506); `primer-cli/src/main.rs` (1349) is the hardest.
- **Recipe that worked for #409:**
  1. In Python, slice the file at unique `///` anchors, assert the pieces add up to the whole file, and write the new files.
  2. Fix visibility and paths.
  3. Compare the code lines with a normalised diff (drop comments and `use` lines, sort).
  4. Compare test counts with `git stash`.

### Carried / owner-or-hardware-gated
- **#260:** Android-voice on-device acceptance; also check that a denied mic keeps the text session (#369).
- **#192:** macOS-native STT + injected TTS smoke.
- **#170 Stages B, E and F:** the OpenRAIL-M disclosure ships before any default flip.
- **#166 item #1:** WhisperStream reuse smoke.
- **#135:** glib (blocked on Tauri 3).
- **Other:** QNN token stability, NPU tuning, latency-routing calibration.

## Open decisions / risks

### This session (#410)
- **A transient blip on a local primary now fails over at once** to the secondary (often cloud) instead of retrying locally. More turns can reach the cloud on a flaky local server. The `--fallback-backend` / router opt-in is the consent, but the owner should confirm the trade-off (it is flagged in the PR).
- **Under the router, the cap follows leg order, not leg identity.** In `CloudPreferred` mode, or for a high-score hybrid turn, the *cloud* leg gets one attempt and local keeps the full budget.
- **The pin is a request count, not wall-clock.** The ceiling is now one full budget: ~1.75 s of backoff, or up to 2 × 5 s with `Retry-After`.
- **QNN and llamacpp ignore the cap** (they have no retry loop). Only the three HTTP backends read it.
- **`GenerationParams` is `Serialize`/`Deserialize`:** the new field defaults to `None` when absent.

### This session (#409)
- **No behaviour change.** It is still device-unverified: the Genie FFI path only runs on Android.

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
# Open at close: #409, #410, the handoff PR. After merge: gh issue view 312 --json state
S=/private/tmp/primer-scratch; mkdir -p "$S"   # or the session's scratchpad dir

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done

# === #410 regression checks ===
~/.cargo/bin/cargo test -p primer-inference --lib -- fallback router failover_leg_cap
~/.cargo/bin/cargo test -p primer-core --lib -- retry gen_params
# === #409: qnn arm (host + android; NDK env for ring) ===
~/.cargo/bin/cargo test -p primer-inference --features qnn 2>&1 | grep '^test result'
B=~/Library/Android/sdk/ndk/29.0.14206865/toolchains/llvm/prebuilt/darwin-x86_64/bin; CC_aarch64_linux_android=$B/aarch64-linux-android24-clang AR_aarch64_linux_android=$B/llvm-ar CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$B/aarch64-linux-android24-clang ~/.cargo/bin/cargo clippy -p primer-inference --features qnn --target aarch64-linux-android --all-targets -- -D warnings

# === #404 proof: on the handoff PR ===
gh pr checks <n>   # expect: detect code changes = pass; every other CI job = skipping
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

- **#409:** a housekeeping change. The code that talks to the phone's AI chip is now in three smaller files instead of one large one. Nothing behaves differently.
- **#410:** when a backup model is configured and the main one is down or busy, the Primer used to keep retrying the main one for up to ~10 seconds before trying the backup, and then retry the backup for up to ~10 more. A child could wait ~20 seconds in silence. Now the main model gets one try, then the backup takes over straight away. Closes #312.
