# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-10-29 (UTC).

**Three PRs are open against `main`** from this session, plus one Dependabot PR:

| PR | Branch | What | Closes |
|---|---|---|---|
| **#380** | `fix/voice-cancel-persists-toggle-372` | Consent-modal Cancel persists `voice_mode_enabled=false` | #372 |
| **#381** | `fix/reembed-force-377` | `reembed_kb --force` re-embeds every passage (+ README) | #377 |
| **#382** | `fix/atomic-seed-load-308` | Seed loading is all-or-nothing (this brief rides on it) | #308 |
| **#379** | Dependabot | rustls 0.23.45 → 0.23.46 (patch), CI all green | — |

- **Merge order:** #381 and #382 both touch `primer-kb-load/src/lib.rs`, in different regions. Expect a trivial rebase for whichever merges second. #380 is independent.
- **`main` is at `b46ffc4`** (#376).

**Since the last brief:**
- **#374, #375 and #376 merged**; #309, #310 and #311 are CLOSED.
- The `sanitize_fts_phrase` hoist (`0ec2481`) rode in with #376's squash; **#378 is CLOSED**.

## What we shipped this session

### #380 (`5211999`): closes #372
- **Problem:** `stop_voice_mode_inner` returned before its toggle flip when no loop was running. After an `AssetMissing` start no loop ever runs, so the consent modal's Cancel never made "not now" durable.
- **Fix:** new `stop_voice_mode_by_user` (`commands/voice/stop.rs`): tear down any loop with the toggle preserved, then `persist_toggle_off` unconditionally. Both the desktop and Android stop commands use it. `start_voice_mode`'s own teardown still uses `_inner(.., false)`, so a failed start keeps the toggle.
- **Test:** `stop_without_a_loop_persists_the_toggle_off` drives the real command via `mock_app`: the toggle is off in memory **and on disk**, and the same text session survives. Red before the fix.

### #381 (`a3bda91`): closes #377
- **Problem:** `force` only warned and then backfilled missing rows. Also, the stub's model id is constant across dims, so a same-name model at a new dim could never re-register. The dim-mismatch errors pointed users at that very command.
- **Fix:** new `SqliteKnowledgeBase::clear_embeddings()`. In one transaction it deletes the locale's vectors and prunes `embedding_models` rows no `embeddings_*` table references. `reembed_kb` calls it first under `force`.
- **Tests:** knowledge (two locales on one DB: the unused model re-registers at a new dim, a model still in use stays pinned) and kb-load (dim-8 → `force` dim-16, the issue's acceptance). Mutation-checked.
- **Docs:** README `reembed_kb` paragraph, CLAUDE.md kb-load bullet.

### #382 (`afce67d` + this brief's docs commit): closes #308
- **Problem:** a malformed line left earlier passages committed and no `sources` rows. The KB was then non-empty, so `auto_seed_if_empty` never retried. Also, `insert_passage` was two autocommitted statements.
- **Fix:**
  - `load_jsonl` = `parse_jsonl` + `write_seed`.
  - New public `load_seed_files` parses **all** files before writing any; `auto_seed_if_empty` calls it.
  - Sources are written before passages.
  - New `SqliteKnowledgeBase::insert_passages` writes a batch in one transaction; `insert_passage` reuses it.
- **Tests:** four in total: a bad line in one file, a bad file among several, batch rollback via a `RAISE(ABORT)` trigger, and content/FTS rollback via a dropped FTS table. Mutation-checked.
- **Docs:** CLAUDE.md `primer-knowledge` bullet, `docs/devel/04`.

### Gate
- **Workspace:** `cargo test --workspace` on the #382 branch: 1398/0.
- **Per touched crate:** test, clippy `--all-targets -D warnings`, rustdoc `--document-private-items` and fmt, all clean.
  - **#380:** `primer-gui` on default / `speech` / `android-native`.
  - **#381 and #382:** `primer-knowledge` and `primer-kb-load`, plus clippy `primer-kb-load --features fastembed`.
- **Not run:** the full rustdoc feature-arm loop (no other feature-gated crate touched) and the fastembed hybrid benchmarks (still owed from #376).

> SHAs are branch-tip commits as pushed; squash-merge gives new ones on `main`.

### Per-session sweep
- **Inline-test detector:** clean for the eighteenth session running.
- **Oversized list:** unchanged. `primer-kb-load/src/lib.rs` is now 443 lines on #382; it will be about 450 once #381 is rebased onto it.

## What's next (concrete acceptance criteria)

### 0. Owner review/merge: #380, #381, #382, #379
- **Acceptance:** CI green, all merge (rebase the second of #381/#382); then `gh issue view 308|372|377 --json state` are all CLOSED.

### 1. Sweep + inline-test detector: re-run EVERY session
Commands are in the resume block.

### 2. Bugs (host-completable; TDD each)
- **Smallest next picks:**
  - **#313** stream buffers drop a final line/event not terminated by `\n` at EOF. **Acceptance:** for each buffer (`SseBuffer`, `NdjsonBuffer`, `OpenAiSseBuffer`), a test feeds a terminal `finish_reason: length` event with no trailing newline, then EOF, and the event is still parsed. The fix is one flush-at-EOF helper per buffer (or one shared one).
  - **#314** `resume_session` doesn't drain pending background tasks. **Acceptance:** a `DialogueManager` test: spawn a slow stub extraction, `resume_session` a different session, then advance; the stale extraction is NOT applied to the resumed learner state.
  - **#307** the vector leg has no similarity floor. Needs a consts value plus a sweep-backed default. Read the issue first; it may need an owner call on the floor value.
- **#371** (voice-loop abort on stop timeout) needs a `JoinHandle::abort` design note first.
- **#352 remaining:** item 2 needs the owner decision below; items 1 and 4 need a design note before code.
- **Older:** #312, #315, #316, #325/#326, #362, #364, #365. Read each before choosing.

### 3. Production-code splits (owner-approved lane, all feature-gated, so dual-verify)
- **`primer-inference/src/qnn/genie/real.rs`** (566 lines, `qnn`) is the recommended one. Baselines: `cargo test -p primer-inference`; `cargo clippy -p primer-inference --features qnn --all-targets -- -D warnings`; the rustdoc `qnn` arm.
- **`primer-speech/src/macos/{tts.rs 668, stt.rs 504}`** (`macos-native`).
- **Low value:** `state_machine/inner.rs` (506). **Hardest:** `primer-cli/src/main.rs` (1353).

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

### This session
- **#381: a `force` run is destructive up front.** It clears the locale's vectors *before* embedding. If the embedder fails part-way (server down), hybrid degrades towards BM25 until a rerun (with or without `force`) completes it. Documented on `reembed_kb`.
- **#381:** a model still used by **another locale** stays pinned at its old dim. Re-embedding under that name at a new dim needs `--force` on every locale that uses it. The error still says so, but per locale.
- **#382:** a *write* failure (disk full) in a later seed file still leaves earlier files committed and the KB non-empty (`shortcut:` comment in `load_seed_files`). Parse errors, the realistic failure, are fully covered.
- **#382:** `insert_passage` is now one transaction per row, which changes the `#[ignore]`'d `bulk_insert_bench` baseline a little (it is in-memory, so the effect is small). It was not re-run.
- **#380:** with a loop running, the user-stop path saves the toggle at the end, after the session drop, instead of mid-teardown. No observable difference; not checked in a live `cargo tauri dev` session.
- **Still owed from #376:** the fastembed hybrid benchmarks:
  `~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test retrieval_quality_hybrid --test retrieval_quality_hybrid_de` (downloads ~570 MB BGE-M3 on first run).

### Carried from earlier sessions
- **#374** turns a corrupt `embedding_models` row into a hard retrieval error, not BM25-only results. That is intended.
- **#369 residual:** a session-build failure after the close still loses the text session (`shortcut:` in `start.rs`).
- **#370:** v9 drop/rename runs with `foreign_keys = ON`; revisit if a future table ever FKs to `turn_classifications`.
- **Mislabelled rows from before #363:** rows in `turn_classifications` / `comprehension_classifiers` from earlier degraded sessions keep the wrong id (#352 item 4).
- **#360 permissions:**
  - Existing `0644` DB files are protected only by the `0700` parent directory.
  - The tighten step runs only when a session opens.
  - Windows and Android are untouched.
- **GUI logging:** log rotation runs only at startup (`shortcut:` in `log_sink/mod.rs`), and the log file may hold child text.
- **Notice text is English-only** and operator-facing. The Settings paths are hand-written strings.
- **Stale CMake cache after Xcode/SDK upgrades:**
  1. `find src/target -name CMakeCache.txt -maxdepth 6 | xargs grep -l 'MacOSX<old>'`
  2. `cargo clean -p <those sys crates>`
- **The `github` MCP server fails to connect** (bad Authorization header). The `gh` CLI works.
- **zsh gotchas:**
  - `$var` is not word-split; use `${=spec}`.
  - **Quote `echo ===X`**; an unquoted `===` errors with "== not found".
  - Use `git grep … -- '*.rs'`.
- **Cargo stops at the first failing test crate.** When mutation-checking across two crates (`-p a -p b`), re-run the second crate alone before concluding its test missed the mutant.

## Patterns to reuse, not reinvent

- **JS behaviour without a JS runner:** use an `include_str!` contract test in `primer-gui/src/*_contract.rs`. Slice the function body by a unique anchor string and assert the call is there. See `voice_stop_contract.rs` and `diagnostics_toggle_contract.rs`. Mutation-check by stashing the `ui/` changes and re-running.
- **Cross-file JS hooks:** app.js exports `window.primer*` functions at the bottom; voice.js lists the ones it uses in its header comment. Add new hooks in both places.
- **Migration tests:** pin *behaviour* (cascade fires, UNIQUE rejects), not DDL text. Build `fresh_vN_conn()` by chaining the earlier helpers. Fixtures should call the real `apply_vN_migrations`; never hand-roll the DDL.
- **Effective backend:** use `primer_engine::served_identity(...)` everywhere. Never re-derive it from `config.backend.kind`.
- **End-to-end degraded-primary GUI tests:** pair a `qnn` primary with a `cloud` fallback and an Inline key (`primary_unavailable_cloud_fallback_config()`).
- **Misbehaving-embedder tests:** wrap `StubEmbedder` and alter its output (`DropsLastEmbedder` in `primer-kb-load/src/tests.rs`). `Named` lives at `primer_core::speech::Named`.
- **Corrupt-row DB tests:** register a row, then `UPDATE … SET <int col> = 'not-a-number'` to force a read-side type error (`conn_with_corrupt_dim`).
- **Forcing a mid-transaction write failure (new):** open a second `Connection` on the same temp file and add `CREATE TRIGGER … BEFORE INSERT … WHEN NEW.id = 'bad' BEGIN SELECT RAISE(ABORT, 'x'); END`, or `DROP` the FTS table. See `insert_passages_rolls_back_*` in `primer-knowledge/src/tests.rs`.
- **Mutation-check a transaction:** remove the transaction entirely. Writing through `&conn` while a `BEGIN` is still open on the same connection is NOT a valid mutant: those writes are still inside the transaction.
- **Env-var-dependent code:** inject `impl Fn(&str) -> Option<String>`; never use `set_var`. For seed discovery, test `load_seed_files(kb, &paths)` directly instead of setting `PRIMER_SEED_DIR`.
- **Mutation-check new tests:**
  1. Back up the file to the scratchpad.
  2. Apply the mutation with `perl -0pi`.
  3. Run the filtered tests.
  4. Restore the file.
- **Python `sub()` edits:** a helper that asserts the match count stops before the write.
- **Bug-fix recipe:**
  1. Branch off `origin/main`, and check `git branch --show-current` **before** editing; this session nearly committed #377 onto the #372 branch.
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
# Open at close: #379 (Dependabot), #380, #381, #382. After merge: for i in 308 372 377; do gh issue view $i --json state; done
S=/private/tmp/primer-scratch; mkdir -p "$S"   # or the session's scratchpad dir

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done

# === Pick the next bug ===
gh issue list --state open --limit 40

# === Standard workspace gate (one cargo pass at a time) ===
~/.cargo/bin/cargo test --workspace 2>&1 | tee $S/ws.log | tail -3
grep -E 'test result: FAILED|^error' $S/ws.log  # expect empty
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo clippy -p primer-gui --features speech --all-targets -- -D warnings
~/.cargo/bin/cargo clippy -p primer-gui --features android-native --all-targets -- -D warnings
~/.cargo/bin/cargo fmt --all -- --check

# === Rustdoc gate (CI-enforced); feature arms mirror ci.yml ===
RUSTDOCFLAGS="-D warnings" ~/.cargo/bin/cargo doc --workspace --no-deps --document-private-items
for spec in "-p primer-speech --features voice-loop,silero,whisper,piper,cpal,supertonic" "-p primer-speech --features android-native" "-p primer-speech --features voice-loop,android-native" \
  "-p primer-inference --features qnn" "-p primer-inference --features llamacpp" "-p primer-gui --features speech" "-p primer-gui --features android-native" \
  "-p primer-cli --features speech" "-p primer-speech --features voice-loop,silero,cpal,macos-native" "-p primer-speech --features voice-loop,silero,cpal,macos-native-26"; do \
  RUSTDOCFLAGS="-D warnings" ~/.cargo/bin/cargo doc ${=spec} --no-deps --document-private-items >/dev/null 2>&1; echo "$spec exit=$?"; done
```

## Reporting back

- **#380:** choosing "not now" on the voice-download prompt now sticks; the prompt no longer reappears every launch. Closes #372.
- **#381:** `primer-kb-load --reembed --force` now really re-embeds the whole knowledge base, so switching embedding models works as the error messages describe. Closes #377.
- **#382:** a broken line in a seed file no longer leaves the knowledge base half-loaded for good; nothing from the bad load is kept and the next start tries again. Closes #308.
