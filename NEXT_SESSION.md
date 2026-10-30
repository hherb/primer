# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-10-30 11:21 (UTC).

**One PR is open against `main`** from this session:

| PR | Branch | What | Closes |
|---|---|---|---|
| **#394** | `fix/embed-default-kb-388` | Default KB is an embedded per-model cache; the gate keeps un-embedded BM25 hits (this brief rides on it) | #388, #389 |

- **`main` is at `86e0ecd`** (#390).

**Since the last brief:** #390 merged, #386 is CLOSED. This session filed #392 and #393.

## What we shipped this session

### #394 (`8b5603a` + this brief's docs commit): closes #388 and #389
- **Owner decisions (asked at session start):**
  - #388: option (a), embed into a persistent per-model cache;
  - #389: keep the BM25 hits of un-embedded passages (not "gate only at full coverage").
- **#388:**
  - `primer_engine::resolve_knowledge_db(explicit, home, embedder_model_id, no_persist) -> KnowledgeDb { InMemory, Explicit, Cache }` (pure, in `paths.rs`).
    - With an embedder wired, the default KB is `~/.primer/kb/<slug(model_id)>.db`.
    - An explicit path always wins, even under `no_persist`.
    - With no embedder, no home, or `no_persist`, it stays `:memory:`.
  - `primer_kb_load::prepare_kb(kb, seed_files, top_up, embedder)`:
    - it seeds when the KB is empty, or always with `top_up`, which the cache passes so a grown corpus reaches it;
    - it then runs `reembed_kb(.., DEFAULT_EMBED_BATCH_SIZE = 16)`;
    - an embed failure only `warn!`s.
  - An explicit path is passed `embedder = None`: seeded when empty, never embedded at startup (unchanged).
  - **CLI:** the KB block moved below the embedder construction in `main.rs`. It prints "Embedding the knowledge base…" when work is pending.
  - **GUI:** `build_with_strategy` builds subsystems *before* `open_stores`, which now takes `embedder`.
  - **Doc fix:** the GUI `PersistenceConfig::no_persist` doc used to say `knowledge_db` is ignored, but the code never ignored it. The doc now matches the code.
- **#389:** `knn_scan` now returns `(hits, embedded_ids)`. When the gate fires, BM25 hits on passages with no vector survive (stopwords dropped, `KB_BM25_ONLY_MIN_SCORE` floor, `final_top_k` cap). On a fully embedded KB that set is empty.
- **Measured** (real CLI, BGE-M3, sandboxed HOME):
  - first run 106 s, i.e. 91 EN passages seeded + embedded (≈640 s of CPU);
  - second run 4.7 s;
  - "can we stop now" now hits the gate on a default run (best cosine 0.43 < 0.45, 0 passages). Before, the gate never fired there.
- **Tests:**
  - #389 KB test (red first; mutation-checked);
  - `resolve_knowledge_db` × 3;
  - `prepare_kb` × 3 (mutation-checked: without `top_up`, and with errors propagated);
  - 4 GUI stage tests that drive the shipped `open_knowledge_base` (mutation-checked by never embedding).
  
  GUI test configs that built sessions from `GuiConfig::default()` now set `embedder.kind = "none"`. `primer-gui` tests went from 446 s to 1.4 s.
- **Docs:**
  - CLAUDE.md: `--knowledge-db` flag text, FTS5 bullet, gate bullet (+ #389), KB-default bullet, `primer-kb-load` bullet;
  - README: `primer-kb-load` section and the CLI flag table;
  - `--knowledge-db` help text.
  
  ROADMAP: no change (bug fix).

### Gate
- `cargo test --workspace`: 1469/0.
- clippy `-D warnings`:
  - workspace `--all-targets`;
  - `primer-gui` `speech` and `android-native`;
  - `primer-kb-load --features fastembed`.
- fmt; workspace rustdoc.
- fastembed: `off_topic_gate`, `retrieval_quality_hybrid{,_de}`. BM25: `retrieval_quality{,_de}`, `off_topic_bm25`. All green.
- **Not run:**
  - the rustdoc feature arms (no feature-gated code touched);
  - the 54-cell hybrid sweeps;
  - the BM25 tripwire (BM25 ranking unchanged).

> SHAs are branch-tip commits as pushed; squash-merge gives new ones on `main`.

### Per-session sweep
- **Inline-test detector:** clean for the twenty-second session running.
- **Oversized list:**
  - new entry `primer-gui/src/wiring/stage_tests.rs` (568, a test file);
  - `primer-cli/src/main.rs` grew from 1353 to 1386.

## What's next (concrete acceptance criteria)

### 0. Owner review/merge: #394
- **Acceptance:** CI green, merged; then `gh issue view 388 --json state` and `gh issue view 389 --json state` are both CLOSED.
- **Owner check:** start the desktop GUI once on a machine without `~/.primer/kb/`. The first session takes ~100 s with no progress shown (#393). Decide whether #393 must land before the next release.

### 1. Sweep + inline-test detector: re-run EVERY session
Commands are in the resume block.

### 2. #393: GUI first-run feedback for the KB embed (recommended next; host-completable)
- **Option 1:** emit a `primer://kb/embedding { done, total }` event from the session build; the frontend shows it in the picker/loading state.
- **Option 2:** open the session at once and embed in the background (retrieval is BM25 + #389 until vectors land). This contends on the KB connection mutex, so it needs a design note first.
- **Recommendation:** option 1. `prepare_kb` would take a progress callback (or the GUI calls `reembed_kb` per batch).
- **Acceptance:**
  - a test that the event fires with `total = passages_missing_embedding().len()` and ends at `done == total`;
  - a contract test for the JS handler (the `include_str!` pattern);
  - a manual first-run check.

### 3. #392: seed edits never reach an existing cache (host-completable)
- **Acceptance:** an edited seed passage (same id, new text) replaces the cached row and drops its vector, so the embed step refills it. Test it via `prepare_kb` with two JSONL versions.
- **Sketch:** a content hash per passage. Needs a KB schema bump (user_version 4 → 5) with the idempotent-migration pattern.

### 4. Other bugs (host-completable; TDD each)
- **#371** (voice-loop abort on stop timeout) needs a `JoinHandle::abort` design note first.
- **#352 remaining:** item 2 needs the owner decision below; items 1 and 4 need a design note before code.
- **Older:** #312, #315, #316, #325/#326, #362, #364, #365. Read each before choosing.

### 5. Production-code splits (owner-approved lane, all feature-gated, so dual-verify)
- **`primer-inference/src/qnn/genie/real.rs`** (566 lines, `qnn`) is the recommended one. Baselines: `cargo test -p primer-inference`; `cargo clippy -p primer-inference --features qnn --all-targets -- -D warnings`; the rustdoc `qnn` arm.
- **`primer-speech/src/macos/{tts.rs 668, stt.rs 504}`** (`macos-native`).
- **Low value:** `state_machine/inner.rs` (506). **Hardest:** `primer-cli/src/main.rs` (1386).

### Carried / owner-or-hardware-gated
- **#260:** Android-voice on-device acceptance; also re-check on device that a denied mic keeps the text session (#369).
- **#192:** manual macOS-native STT + injected TTS smoke.
- **#170 Stages B, E and F:** the OpenRAIL-M disclosure must ship before any default flip.
- **#166 item #1:** owner-run WhisperStream reuse smoke.
- **#135:** glib (blocked on Tauri 3).
- **Branch protection:** make `cargo test (default features)` a required check.
- **Other:** QNN token stability, NPU tuning, latency-routing calibration.

## Open decisions / risks

### #352 item 2: what counts as a pointless fallback? (owner decision, unchanged)
`--fallback-backend == --backend` with a different **model** is legitimate (small/big ollama models, or hybrid routing on one server).
- (a) notice only when kind **and** model match;
- (b) kind-only, worded as "no protection if the server goes down";
- (c) drop the item and keep the `tracing::warn!`.

**Recommendation:** (a), and downgrade or drop today's kind-only warn on the router path.

### This session (#394)
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
# Open at close: #394. After merge: gh issue view 388 --json state; gh issue view 389 --json state
S=/private/tmp/primer-scratch; mkdir -p "$S"   # or the session's scratchpad dir

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done

# === #393 starting point ===
gh issue view 393; gh issue view 392
git grep -n "prepare_kb\|fn reembed_kb" -- '*.rs' | grep -v tests
~/.cargo/bin/cargo test -p primer-gui knowledge_base
~/.cargo/bin/cargo test -p primer-kb-load --lib prepare_kb

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

- **#394:** the Primer's semantic knowledge search now works out of the box. Before, the default setup never prepared the encyclopedia passages for it, so search quietly fell back to keyword matching, and the filter that keeps small talk from pulling in passages never ran. Now the first start prepares them once and saves the result. That takes about a minute and a half on a Mac; later starts take seconds. "Can we stop now" no longer pulls passages into the prompt. Closes #388 and #389.
