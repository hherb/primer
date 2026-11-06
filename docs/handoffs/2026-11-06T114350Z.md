# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-11-06 (UTC).

**Two PRs are open against `main`:**
- **#403** (`fix/stub-subsystem-model-warn-365`) closes #365 and #362. This brief rides on it.
- **#405** (`fix/kb-cache-per-seed-dir-401`) closes #401 and adds the `finalcheck` skill.

The two touch different files and merge in either order.

- **`main` is at `7e8f2c5`** (#402).

**Since the last brief:** #402 merged; #395 is CLOSED.

## What we shipped this session

### #403 — `87dd4b6` + this brief's commit: closes #365 and #362
- **#365:** a new pure `stub_ignores_model(served_kind, backend_override, model) -> Option<&'static str>` in `primer-engine/src/wiring/subsystems.rs` replaces the three duplicated "model ignored" warn blocks in `build_classifier` / `build_extractor` / `build_comprehension`.
  - It now also warns on the `("stub", None)` arm: a stub primary, or a stub fallback serving alone under `PrimaryUnavailable`. That override used to vanish silently.
  - Behaviour is unchanged (the stub still wins); only the log line is new. The message is `--<subsystem>-model ignored: <reason> (deterministic, no model)`.
- **#362:** `classifier_without_override_classifies_through_main_backend` (in `wiring/classifier_construction_tests.rs`) gives `build_classifier` a canned-JSON main backend under an `openai-compat` served kind. It asserts `classify` returns the canned assessment, which a freshly built backend could not produce.
  - The pin lives in the engine, where `build_classifier` is, not in the GUI `stage_tests.rs` the issue named. The GUI test only reaches the same function.
- **Tests:** 4 for the helper, plus 1 for inheritance. Mutation-checked: dropping the `("stub", None)` arm fails one; swapping `Arc::clone` for `build_backend` fails the other.
- **Deps:** `primer-engine` gained dev-deps `async-trait` + `futures` (for the test double).
- **Docs:** README, ROADMAP and CLAUDE.md: no change (log line plus tests only).

### #405 — `ae098a5` + `1f7b453`: closes #401; tracks the project skills
- **#401 (your option (a)):** the KB cache is now `~/.primer/kb/<slug(model)>-<fnv1a-64 hex of the canonical seed dir>.db`, or `<slug>.db` when no seed file is found.
  - New `primer_engine::seed_corpus_dir(seed_files)`; `resolve_knowledge_db` takes `seed_dir`. The CLI and the GUI (`wiring/stores.rs`) discover the seed files once and pass them to both calls.
  - The hash is FNV-1a, not std's hasher, so the file name survives Rust upgrades. A test pins the literal (`bge-m3-07d65507b49cba97.db` for `/s`).
  - The GUI stage test `knowledge_base_with_embedder_is_an_embedded_per_model_cache` now resolves with the seed dir, so a caller passing `None` fails it (mutation-checked).
  - Docs: the CLAUDE.md `--knowledge-db` + hybrid-gate bullets, the README CLI flag table, and the CLI `--knowledge-db` doc comment.
- **Skills:** `.gitignore` is now `.claude/*` + `!.claude/skills/`, so `finalcheck`, `fixall` and `nextsession` are tracked. Settings and worktrees stay local.
  - `finalcheck` was adapted from `~/src/secretary`: primer's `ci.yml` jobs with local repro commands, an explicit `origin/main` merge before `fixall` (ours doesn't merge), the NEXT_SESSION/handoff exact-copy rule, `cancel-in-progress` semantics, the docs-only gap (#404), and the checks CI doesn't run.
- **Filed #404:** branch protection already requires `cargo test (default features)`, but `ci.yml`'s workflow-level `paths-ignore` means a docs-only PR never posts it.
- **Gate:** `RUSTFLAGS=-D warnings cargo test --workspace --no-fail-fast` 1502/0. Clippy `-D warnings` on the workspace, `primer-gui --features speech`, `primer-cli --features speech` and `primer-gui --features android-native`; fmt; rustdoc. The fastembed recall tests were not run (no retrieval code changed).

### Gate (#403)
- `cargo test --workspace`: 1505/0.
- clippy `-D warnings`: workspace `--all-targets` and `primer-gui --features speech`.
- fmt and workspace rustdoc.
- **Not run:** the other clippy feature arms and the rustdoc feature-arm loop. Only `primer-engine` default-feature code changed.

> SHAs are branch-tip commits as pushed; squash-merge gives new ones on `main`.

### Per-session sweep
- **Inline-test detector:** clean for the twenty-seventh session running.
- **Oversized list:** unchanged. Production files over 500 lines: `primer-cli/src/main.rs` 1378, `primer-speech/src/macos/tts.rs` 668, `primer-inference/src/qnn/genie/real.rs` 566, `state_machine/inner.rs` 506, `primer-speech/src/macos/stt.rs` 504.

## What's next (concrete acceptance criteria)

### 0. Owner review/merge: #403 and #405
- **Acceptance:** CI green and both merged. Then `gh issue view <n> --json state` shows CLOSED for 365, 362 and 401.
- Try `/finalcheck` on one of them: it is new, so watch whether its repro commands match what CI runs.

### 1. Sweep + inline-test detector: re-run EVERY session
Commands are in the resume block.

### 2. #404: docs-only PRs vs the required check
- **Acceptance:** a PR touching only `*.md` gets a passing `cargo test (default features)` (or skipped, which counts as passing) without running the heavy jobs.
- Also refresh the stale `ci.yml` header comment that says branch protection "is not yet wired".
- This is a CI-config change, so confirm the approach with the owner first (`dorny/paths-filter` + job-level `if:` is the recommendation in the issue).

### 3. Manual first-run GUI check for #396 (owner)
Still not watched in the real webview: `mv ~/.primer/kb ~/.primer/kb.bak`, launch the GUI, Start new → Save & start; the button should count "… 0 / 91" upward.

### 4. Other bugs (host-completable; TDD each)
- **#364:** a CLI test seam for subsystem wiring under `PrimaryUnavailable`. It is the closest to this session's work.
- **#371:** needs a `JoinHandle::abort` design note first.
- **#352 remaining:** item 2 needs the owner decision below; items 1 and 4 need a design note first.
- **Older:** #312, #315, #316, #325/#326. Run `gh issue list` to pick; read each before starting.

### 5. Production-code splits (owner-approved lane, all feature-gated, so dual-verify)
- **`primer-inference/src/qnn/genie/real.rs`** (566, `qnn`) is the recommended one; then `primer-speech/src/macos/{tts.rs, stt.rs}`; `primer-cli/src/main.rs` (1378) is the hardest.

### Carried / owner-or-hardware-gated
- **#260:** Android-voice on-device acceptance; also check that a denied mic keeps the text session (#369).
- **#192:** macOS-native STT + injected TTS smoke.
- **#170 Stages B, E and F:** the OpenRAIL-M disclosure ships before any default flip.
- **#166 item #1:** WhisperStream reuse smoke.
- **#135:** glib (blocked on Tauri 3).
- **Other:** QNN token stability, NPU tuning, latency-routing calibration.

## Open decisions / risks

### This session (#403)
- **The new warn fires on a stub primary run with a `--*-model` override.** Before, that run was silent. Under the default `RUST_LOG` it shows on stderr. Nothing in-repo passes that combination.
- **The warn text now says "the subsystem backend is stub" / "the served backend is stub"** instead of naming `--<subsystem>-backend`. Anything grepping logs for the old wording would miss it. Nothing in-repo does.
- **#362's pin covers `build_classifier` only.** Extractor and comprehension were already pinned by their ids (`llm:<backend>:<model>`).

### #405 (#401)
- **Upgrade cost:** existing `~/.primer/kb/<model>.db` caches are left in place (safe to delete) and orphaned. The first start after upgrading re-embeds into the new file: ~100 s on BGE-M3 for EN, again for DE. No automatic adoption: renaming a live SQLite file with sidecars wasn't worth the risk for a rebuildable cache.
- **Orphans accumulate**, one per seed dir ever used (a few MB each). Nothing cleans them.
- **A moved checkout or a renamed app bundle is a new folder**, so it costs one full re-embed.
- **No seed files found** falls back to `<model>.db`, the legacy name. With an empty corpus `prepare_kb` prunes nothing, so a legacy cache stays intact.
- **The CLI's caller has no test seam** (same as #364). Only the GUI caller is pinned.

### #401: how should the KB cache be keyed? (DECIDED: (a), shipped in #405)
Today the cache file is `~/.primer/kb/<slug(model)>.db`. Two seed corpora on one machine (an installed `.app` plus a dev checkout) prune and re-embed each other on every switch. Nothing is lost, but each switch re-embeds the differing passages.
- (a) Key by model **plus a short hash of the canonical seed-dir path**. Each corpus location gets its own cache. Edits in place still go through the incremental #392/#398 path. Old caches pile up, one per seed dir, a few hundred KB each.
- (b) Key by model plus a **content** hash, as the issue suggests. Every corpus edit then orphans the whole cache and re-embeds all ~91 passages (~100 s on BGE-M3), which throws away #392's incremental replace.
- (c) Accept the flip as designed; the `prepare_kb` docs already say the cache mirrors the discovered corpus.

**Owner chose (a)**, shipped in #405.

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
- **The cache is per machine and per model, and holds no child data.** It is created with `create_private_dir_all` anyway. Deleting `~/.primer/kb/` is safe: it rebuilds on the next open.
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
# Open at close: #403, #405. After merge: for n in 365 362 401; do gh issue view $n --json state; done
S=/private/tmp/primer-scratch; mkdir -p "$S"   # or the session's scratchpad dir

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done

# === #403 / #405 regression checks ===
~/.cargo/bin/cargo test -p primer-engine --lib classifier_construction
~/.cargo/bin/cargo test -p primer-engine --lib paths
~/.cargo/bin/cargo test -p primer-gui --lib knowledge_base

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

- **#403:** if the main model is the built-in test stand-in and someone asks for a specific model for one of the helper checks (engagement, concepts, comprehension), the Primer used to drop that request without a word. Now it logs a warning. A new test also proves the engagement check shares the main model, which is how it inherits the cloud fallback. Nothing changes for the child. Closes #365 and #362.
- **#405:** the built-in library's saved cache is now kept separately for each copy of the library on the computer, so an installed app and a developer copy no longer overwrite each other's work. Expect one slow first start after updating while it rebuilds. The PR also adds a `/finalcheck` command for the final review-and-CI pass before merging. Closes #401.
