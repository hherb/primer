# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-10-24 (UTC).

**Two PRs are open against `main`:**

| PR | Branch | What | Closes |
|---|---|---|---|
| **#367** | `fix/voice-stop-header-352` | Return to the session picker after "End voice mode" (this brief rides on it) | #352 item 3 (refs only; the issue stays open) |
| **#368** | `test/v8-migration-shape-332` | v8 migration shape + cascade tests | #332 |

`main` is at `85009d8` (#363).

**Since the last brief:**
- **#363 merged.**
- **#351 is confirmed CLOSED.**

## What we shipped this session

### #367 (`e2e9d27` + this brief's docs commit): #352 item 3

**Problem:** `stop_voice_mode` drops the active session server-side, even when it returns `Err`. `ui/voice.js` only switched the composer back to text. The header pills, `#backend-notice-banner` and an enabled text composer then described a dead session, and typing failed with "no active session — call start_session first".

**Fix (owner-chosen: "back to picker"):**
- After an explicit stop, `onToggleClick` calls `window.primerReturnToPicker`.
- That is app.js `onSwitchSession`, the Sessions-button path: an idempotent `close_session`, a reset of the chat, header and banners, then `showPicker()`.
- The session that just ended can be resumed from the picker.
- The owner rejected "reset header only" and "continue in text".

**Tests:** new `primer-gui/src/voice_stop_contract.rs`, an `include_str!` contract test, 3 tests. They are mutation-checked: 2 fail against the old JS.

**Docs:** CLAUDE.md voice paragraph and README voice bullet.

**Gate:**
- `cargo test --workspace`: 1379/0.
- clippy `-D warnings`: `primer-gui` default and `speech`.
- fmt: clean.
- rustdoc: `primer-gui` default and `speech`.

**Filed:** **#366**. A *failed* `start_voice_mode` closes the text session at step 1, before the steps that can fail:
- step 3, session build;
- step 4, the assets: consent cancel, `auto_download_disabled`;
- step 5, backend init / permission.

The symptom is the same dead-composer, on a different path. The fix is in Rust (reorder the steps). The issue includes a test recipe.

### #368 (`f49eda6`): closes #332

Four new v8 tests in `primer-storage/src/schema/v4_tests.rs`:
1. Tables and index exist after v8.
2. A session delete cascades to `embeddings_turns`.
3. `embedding_models.name` is `UNIQUE`.
4. A re-run is idempotent.

They are mutation-checked: dropping `ON DELETE CASCADE` or `UNIQUE` turns tests 2 and 3 red. The 3 `store/embeddings.rs` fixture tests now call `apply_v8_migrations` instead of a hand-rolled `CREATE TABLE`.

**Gate:** `primer-storage` 158/0; clippy, fmt and rustdoc clean.

> SHAs are the branch-tip commits as pushed; squash-merge will give new ones on `main`.

### Per-session sweep
- **Inline-test detector:** clean for the fifteenth session running.
- **Oversized list:** unchanged.

## What's next (concrete acceptance criteria)

### 0. Owner review/merge: #367, #368
- **Acceptance:** CI is green and both merge.
- Then check: `gh issue view 332 --json state` should be CLOSED. **#352 stays open**: #367 only says "Refs".

### 1. Sweep + inline-test detector: re-run EVERY session
Commands are in the resume block.

### 2. Bugs (host-completable; TDD each)
- **#366** (recommended next, Rust, small):
  - Reorder `primer-gui/src/commands/voice/start.rs` so the asset resolve (needs only `cfg`), the session build and the backend init all run **before** `close_session_inner` / `stop_voice_mode_inner`.
  - **Acceptance:** a `commands/voice/tests.rs` test with an active session plus missing voice assets. `start_voice_mode`'s failure must leave `state.session` as `Some` and the sticky toggle unchanged.
  - Check whether `voice_android.rs` start has the same ordering.
  - **Watch:** the step-3 `build_active_session` opens the same per-child session DB while the old session is still open. SQLite allows two connections, but check that the old session's close/drain doesn't race the new one. You may need to build the session last, after assets and backends.
- **#331** (storage, medium): v9 migration that rebuilds `turn_classifications` with `session_id … ON DELETE CASCADE`.
  - Use the create / copy / drop / rename steps in one tx.
  - Keep `UNIQUE(turn_id, classifier_id)`.
  - Backfill `session_id` from `turns`.
  - **Acceptance:** with classifications present, `DELETE FROM sessions` succeeds and the rows are gone; an idempotent re-run; `USER_VERSION` = 9. Update `user_version_constant_is_eight` and the CLAUDE.md "Schema is at version 8" text.
  - Use #368's cascade test as the template.
- **#352 remaining:**
  - **Item 2 needs an owner decision first** (see Open decisions).
  - Items 1 and 4 (per-turn "served by" signal; subsystem soft-fail signal) need a design note.
- **Older:** #307–#316 (retrieval/storage/streaming), #325/#326 (voice).

### 3. Production-code splits (owner-approved lane, all feature-gated, so dual-verify)
- **`primer-inference/src/qnn/genie/real.rs`** (566 lines, `qnn`) is the recommended one. Baselines:
  - `cargo test -p primer-inference`;
  - `cargo clippy -p primer-inference --features qnn --all-targets -- -D warnings`;
  - the rustdoc `qnn` arm.
- **`primer-speech/src/macos/{tts.rs 668, stt.rs 504}`** (`macos-native`).
- **Low value:** `state_machine/inner.rs` (506).
- **Hardest:** `primer-cli/src/main.rs` (1353).

### Carried / owner-or-hardware-gated
- **#260:** Android-voice on-device acceptance.
- **#192:** manual macOS-native STT + injected TTS smoke.
- **#170 Stages B, E and F:** the OpenRAIL-M disclosure must ship before any default flip.
- **#166 item #1:** owner-run WhisperStream reuse smoke.
- **#135:** glib (blocked on Tauri 3).
- **Branch protection:** make `cargo test (default features)` a required check.
- **Other:** QNN token stability, NPU tuning, latency-routing calibration.

## Open decisions / risks

### #352 item 2: what counts as a pointless fallback? (owner decision)
The issue proposes a third `BackendNotice` whenever `--fallback-backend == --backend`, on both the fallback and the router paths. But a same **kind** with a different **model** is a legitimate setup:
- ollama small model as primary, ollama big model as fallback (resilience against model-not-found);
- small/large hybrid routing on one server.

A kind-only notice would show a warning banner for a valid config.

**Options:**
- (a) notice only when kind **and** model match;
- (b) kind-only, but worded as "no protection if the server goes down";
- (c) drop the item and keep the `tracing::warn!`.

**Recommendation:** (a). Also downgrade or drop today's kind-only warn on the router path.

### #367
- **Ending voice mode now sends the user to the picker.** The voice transcript is cleared from view, but it is saved and can be resumed from the picker.
- **Not checked in a live `cargo tauri dev` session.** The only verification is the contract test, plus `node --check` syntax checks on both JS files.
- **An exit the user did not ask for keeps the session.** `primer://voice/exit` (loop error, keyword "goodbye") does NOT return to the picker. In that case `stop_voice_mode_inner` isn't called, so the session is still alive. Correct today; revisit if exit handling changes.

### Carried from earlier sessions
- **Mislabelled rows from before #363:** rows in `turn_classifications` / `comprehension_classifiers` from earlier degraded sessions keep the wrong id. Under `PrimaryUnavailable`, a model-only override runs on the fallback kind; an invalid model there soft-fails per turn and is only logged (#352 item 4).
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

## Patterns to reuse, not reinvent

- **JS behaviour without a JS runner:** use an `include_str!` contract test in `primer-gui/src/*_contract.rs`. Slice the function body by a unique anchor string and assert the call is there. See `voice_stop_contract.rs` and `diagnostics_toggle_contract.rs`. Mutation-check by stashing the `ui/` changes and re-running.
- **Cross-file JS hooks:** app.js exports `window.primer*` functions at the bottom; voice.js lists the ones it uses in its header comment. Add new hooks in both places.
- **Migration tests:** pin *behaviour* (cascade fires, UNIQUE rejects), not DDL text. Build `fresh_vN_conn()` by chaining the earlier helpers. Fixtures should call the real `apply_vN_migrations`; never hand-roll the DDL.
- **Effective backend:** use `primer_engine::served_identity(...)` everywhere. Never re-derive it from `config.backend.kind`.
- **End-to-end degraded-primary GUI tests:** pair a `qnn` primary with a `cloud` fallback and an Inline key (`primary_unavailable_cloud_fallback_config()`).
- **Env-var-dependent code:** inject `impl Fn(&str) -> Option<String>`; never use `set_var`.
- **Mutation-check new tests:**
  1. Back up the file to the scratchpad.
  2. Apply the mutation with `perl -0pi`.
  3. Run the filtered tests.
  4. Restore the file.
- **Python `sub()` edits:** a helper that asserts the match count stops before the write.
- **Bug-fix recipe:**
  1. Branch off `origin/main`.
  2. Failing test, then fix, then green.
  3. Test + clippy each touched crate, plus its feature arms. Then fmt and rustdoc.
  4. `git grep` the docs for drift.
  5. Commit with `Closes #N`, push, open the PR.
  6. File any out-of-scope half as an issue.
- **Split recipe, substantive-line diff, Dependabot triage:** see `docs/handoffs/2026-10-16T202752Z.md`.
- **Session start:** check `git status` first, and read `NEXT_SESSION.md` AFTER `git pull`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status --short && git checkout main && git pull --ff-only && git log --oneline -6 && gh pr list --state open
# Open at close: #367, #368. After merge: gh issue view 332 --json state   (352 stays open)
S=/private/tmp/primer-scratch; mkdir -p "$S"   # or the session's scratchpad dir

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done

# === Next bug (#366): start here ===
gh issue view 366
git checkout -b fix/voice-start-keeps-session-366 origin/main
sed -n 30,80p crates/primer-gui/src/commands/voice/start.rs
grep -n "fn \|state.session" crates/primer-gui/src/commands/voice/tests.rs | head -40

# === Standard workspace gate (one cargo pass at a time) ===
~/.cargo/bin/cargo test --workspace 2>&1 | tee $S/ws.log | tail -3
grep -E 'test result: FAILED|^error' $S/ws.log  # expect empty
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo clippy -p primer-gui --features speech --all-targets -- -D warnings
~/.cargo/bin/cargo fmt --all -- --check

# === Rustdoc gate (CI-enforced); feature arms mirror ci.yml ===
RUSTDOCFLAGS="-D warnings" ~/.cargo/bin/cargo doc --workspace --no-deps --document-private-items
for spec in "-p primer-speech --features voice-loop,silero,whisper,piper,cpal,supertonic" "-p primer-speech --features android-native" "-p primer-speech --features voice-loop,android-native" \
  "-p primer-inference --features qnn" "-p primer-inference --features llamacpp" "-p primer-gui --features speech" "-p primer-gui --features android-native" \
  "-p primer-cli --features speech" "-p primer-speech --features voice-loop,silero,cpal,macos-native" "-p primer-speech --features voice-loop,silero,cpal,macos-native-26"; do \
  RUSTDOCFLAGS="-D warnings" ~/.cargo/bin/cargo doc ${=spec} --no-deps --document-private-items >/dev/null 2>&1; echo "$spec exit=$?"; done
```

## Reporting back

- **#367:** clicking "End voice mode" now takes you back to the session picker, where you can resume the conversation, instead of leaving a text box that no longer works.
- **#366 (filed):** a voice-mode start that fails (for example, cancelling the model download) still ends your text session. This is next.
- **#368:** the embeddings database migration now has tests proving that deleting a session also deletes its stored vectors. Closes #332.
