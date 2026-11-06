# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-11-06 (UTC).

**Two PRs are open against `main`:**
- **#407** (`fix/cli-subsystem-seam-364`) closes #364. This brief rides on it.
- **#408** (`ci/docs-only-required-check-404`) closes #404 (CI config only).

The two touch different lines (both edit CLAUDE.md, in different paragraphs) and merge in either order.

- **`main` is at `a614395`** (#405).

**Since the last brief:** #403 and #405 merged; #365, #362 and #401 are CLOSED.

## What we shipped this session

### #407 — `92f9ea0` + this brief's commit: closes #364
- **New `primer_engine::build_served_subsystems`** (+ `SubsystemSettings`, `ServedSubsystems`) in `primer-engine/src/wiring/subsystems.rs`. It runs `served_identity` and then the classifier / extractor / comprehension builders, and its `Err(String)` names the failing subsystem (`constructing concept extractor: …`).
  - The CLI (`main.rs`, now 1347 lines, was 1381) and the GUI's `build_subsystems` both call it. The GUI's `build_subsystems` now takes `primary_kind`, `main_model` and `notice`, and returns `(DialogueManagerSubsystems, (served_kind, served_model))` for the sidebar.
  - `wiring/served_subsystems_tests.rs` (3 tests): a synthetic `PrimaryUnavailable` (qnn primary, ollama fallback, `--classifier-model` override, explicit `--comprehension-backend stub`), the no-notice case, and the error naming for all three subsystems. Mutation-checked: dispatching on the primary fails the first test.
- **CLI fix found by the smoke run:** the "every subsystem is set to qnn … serialise" warning fired even when qnn was unavailable and the fallback served. `npu_serialisation_warning` now takes `npu_serves` (false under `PrimaryUnavailable`), which drops only the all-qnn warning; the all-stub feature-loss warning still fires (finalcheck review caught the first version skipping both). Pinned by `fallback_serving_drops_only_the_qnn_warning`.
- **Sweep regression fixed:** #405 pushed `primer-engine/src/paths.rs` past 500 lines with an inline test module. The tests moved to `paths/tests.rs` (count unchanged: 116 + 3 new = 119 in `primer-engine --lib`).
- **Docs:** the CLAUDE.md `FallbackBackend` bullet now names the helper. README and ROADMAP: no change.

### #408 — `aa6bcf6`: closes #404 (owner chose the paths-filter approach)
- `pull_request` no longer has `paths-ignore`. A new `changes` job (`dorny/paths-filter@v3`, `predicate-quantifier: every`) outputs `code`. The five heavy jobs `needs: changes` and are skipped when `code == 'false'`. A skipped job counts as passing for branch protection.
- **The gate fails open:** `if: ${{ !cancelled() && (needs.changes.result != 'success' || needs.changes.outputs.code != 'false') }}`. A failed filter runs everything instead of skipping (and so passing) the required check.
- `push` to `main` is unchanged (it keeps `paths-ignore`; the filter step is skipped there, and its empty output runs every job).
- **Docs:** the `ci.yml` header, the CLAUDE.md branch-protection paragraph, and `finalcheck`'s docs-only note.
- `actionlint` clean. **The docs-only path itself is not yet proven on GitHub** (see item 0).

### Gate (#407)
- `cargo test --workspace --no-fail-fast`: 1511/0.
- clippy `-D warnings`: workspace `--all-targets`, `primer-gui --features speech` and `android-native`, `primer-cli --features speech`, `qnn` and `llamacpp`.
- fmt and workspace rustdoc.
- Real CLI smoke: `--backend qnn --fallback-backend ollama --fallback-model small --classifier-model big` prints `llm:big`, `llm:ollama:small` ×2, and no NPU warning.
- **Not run:** the rustdoc feature-arm loop and the fastembed recall tests (no retrieval or feature-gated code changed).

> SHAs are branch-tip commits as pushed; squash-merge gives new ones on `main`.

### Per-session sweep
- **Inline-test detector:** it caught `primer-engine/src/paths.rs` (fixed in #407). After that, clean.
- **Oversized list:** production files over 500 lines: `primer-cli/src/main.rs` 1347, `primer-speech/src/macos/tts.rs` 668, `primer-inference/src/qnn/genie/real.rs` 566, `state_machine/inner.rs` 506, `primer-speech/src/macos/stt.rs` 504.

## What's next (concrete acceptance criteria)

### 0. Owner review/merge: #407 and #408, then prove #404 on a docs-only PR
- **Acceptance:** CI green and both merged. Then `gh issue view <n> --json state` shows CLOSED for 364 and 404.
- **#404 proof:** the next docs-only PR (the next handoff, if it carries no code) shows `detect code changes` passing, every heavy job **skipped**, and merges without a bypass. If the heavy jobs run instead, or the PR is blocked, reopen #404.

### 1. Sweep + inline-test detector: re-run EVERY session
Commands are in the resume block.

### 2. Manual first-run GUI check for #396 (owner)
Still not watched in the real webview: `mv ~/.primer/kb ~/.primer/kb.bak`, launch the GUI, Start new → Save & start; the button should count "… 0 / 91" upward.

### 3. Other bugs (host-completable; TDD each)
- **#371:** needs a `JoinHandle::abort` design note first.
- **#352 remaining:** item 2 needs the owner decision below; items 1 and 4 need a design note first.
- **Older:** #312, #315, #316, #325/#326. Run `gh issue list` to pick; read each before starting.

### 4. Production-code splits (owner-approved lane, all feature-gated, so dual-verify)
- **`primer-inference/src/qnn/genie/real.rs`** (566, `qnn`) is the recommended one; then `primer-speech/src/macos/{tts.rs, stt.rs}`; `primer-cli/src/main.rs` (1347) is the hardest.

### Carried / owner-or-hardware-gated
- **#260:** Android-voice on-device acceptance; also check that a denied mic keeps the text session (#369).
- **#192:** macOS-native STT + injected TTS smoke.
- **#170 Stages B, E and F:** the OpenRAIL-M disclosure ships before any default flip.
- **#166 item #1:** WhisperStream reuse smoke.
- **#135:** glib (blocked on Tauri 3).
- **Other:** QNN token stability, NPU tuning, latency-routing calibration.

## Open decisions / risks

### This session (#407)
- **The CLI prints its three `Engagement classifier: …` / `Concept extractor: …` / `Comprehension classifier: …` lines after all three are built**, not one at a time. On a failure, the earlier lines no longer print; the error still names the subsystem.
- **`build_served_subsystems` returns `Result<_, String>`**, not `PrimerError`, so both binaries keep their exact error text. A future caller wanting a typed error has to wrap it.

### This session (#408)
- **`dorny/paths-filter@v3` is a third-party action pinned by tag**, like the repo's other actions (no SHA pins). It needs `pull-requests: read` on the `changes` job only.
- **The ignore list now lives in two places:** the `changes` filter and the `push` `paths-ignore`. Keep them in sync; the `ci.yml` header says so.
- **Every PR now pays for one tiny `changes` job** (a few seconds on ubuntu).
- **`validate gradle wrapper` is gated too**, so a docs-only PR skips it. That is fine: it only checks the committed jar.

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
# Open at close: #407, #408. After merge: for n in 364 404; do gh issue view $n --json state; done
S=/private/tmp/primer-scratch; mkdir -p "$S"   # or the session's scratchpad dir

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done

# === #407 regression checks ===
~/.cargo/bin/cargo test -p primer-engine --lib served_subsystems
~/.cargo/bin/cargo test -p primer-gui --lib subsystems

# === #408: after merge, on the next docs-only PR ===
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

- **#407:** the command-line app and the desktop app now set up the background helpers (engagement, concepts, comprehension) through one shared, tested step. If the main model can't start and a backup model takes over, the helpers are guaranteed to use the backup, in both apps. Also, a misleading warning about the phone's AI chip no longer shows when that chip isn't the one in use. Nothing changes for the child. Closes #364.
- **#408:** a pull request that only changes documentation used to get stuck, because the required test check never ran. Now a quick first step spots docs-only changes, the heavy checks are skipped, and the PR can merge. If that first step ever breaks, everything runs as before. Closes #404.
