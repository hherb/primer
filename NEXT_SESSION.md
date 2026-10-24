# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-10-24 (UTC).

**Two PRs are open against `main`:**

| PR | Branch | What | Closes |
|---|---|---|---|
| **#369** | `fix/voice-start-keeps-session-366` | A failed voice-mode start keeps the text session (this brief rides on it) | #366 |
| **#370** | `fix/turn-classifications-cascade-331` | Schema v9: session delete cascades through `turn_classifications` | #331 |

`main` is at `221ee0c` (#368).

**Since the last brief:**
- **#367 and #368 merged.**
- **#332 is confirmed CLOSED.**
- **#352 stays open**, as planned.

## What we shipped this session

### #369 (`eb98d9f` + this brief's docs commit): closes #366

**Problem:** `start_voice_mode` closed the text session and the voice loop before the steps that can fail (assets, backend init). A consent "Cancel", `auto_download_disabled` or a backend error then left a dead-session composer.

**Fix:**
- **Desktop order:** assets → backends → close text session and voice loop → build session → spawn.
- **Why the session build stays after the close:** the close flushes learner-model updates, and the build reads them back. Building earlier would load a stale learner, which later overwrites the fresh one.
- **Residual:** a `shortcut:` comment covers a session-build failure after the close. It needs a config change between the two builds.
- **Android:** `start_voice_mode_android` gets the same reorder. The JNI bridge, the `RECORD_AUDIO` check and the backends now run before the teardown.
- **No JS change:** the consent "Cancel" path calls `stop_voice_mode`, which is a no-op when no loop is running.

**Test seam:**
- Both commands are now generic over `R: tauri::Runtime`; `generate_handler!` infers `Wry`.
- `primer-gui` dev-deps enable `tauri`'s `test` feature.
- `commands/voice/tests.rs` drives the **real** commands via `tauri::test::mock_app`, with shared helpers `app_with_text_session` and `assert_text_session_untouched`.
- Two tests, `speech` and `android-native` arms. Both are mutation-checked: moving the teardown back in front turns them red.

**Gate:**
- `cargo test --workspace`: 1383/0.
- `primer-gui` speech: 296/0; android-native: 252/0.
- clippy `-D warnings`: workspace, gui speech, gui android-native.
- fmt: clean.
- rustdoc: gui default, speech and android-native.

**Docs:** CLAUDE.md voice paragraph.

### #370 (`e5ae2b2`): closes #331

**Fix:** `schema/migrations/v9.rs` rebuilds `turn_classifications` with `turn_id … ON DELETE CASCADE`.
- Create / copy / drop / rename in one transaction, guarded by `pragma_foreign_key_list`, so it runs once.
- **No `session_id` column:** `turns.session_id` already cascades, so this needs no backfill and no insert-path change.
- Carried over: rows, ids, UNIQUE and the index.
- `USER_VERSION` = 9.
- The duplicate `user_version_is_eight` test (`session_tests.rs`) is dropped.

**Tests:** 2 new in `v4_tests.rs`. Mutation-checked: a no-op v9 reproduces the exact FK error.

**Gate:** workspace 1384/0; clippy, fmt and rustdoc (`primer-storage`) clean.

**Docs:**
- CLAUDE.md, now "version 9", with v9 named as the FK-rebuild template.
- `docs/devel/05-storage-and-sessions.md`: new table row, and the recipe shifted to a hypothetical v10.
- The v5 comment.

> SHAs are the branch-tip commits as pushed; squash-merge will give new ones on `main`.

### Per-session sweep
- **Inline-test detector:** clean for the sixteenth session running.
- **Oversized list:** unchanged.

## What's next (concrete acceptance criteria)

### 0. Owner review/merge: #369, #370
- **Acceptance:** CI is green and both merge.
- Then check: `gh issue view 366 --json state` and `gh issue view 331 --json state` should both be CLOSED.
- **Merge-order note:** both PRs edit CLAUDE.md, on different lines. If the second squash conflicts, rebase it.

### 1. Sweep + inline-test detector: re-run EVERY session
Commands are in the resume block.

### 2. Bugs (host-completable; TDD each)
- **#352 remaining:**
  - **Item 2 needs an owner decision first** (see Open decisions).
  - Items 1 and 4 (per-turn "served by" signal; subsystem soft-fail signal) need a design note before code.
- **Older:** #307–#316 (retrieval/storage/streaming), #325/#326 (voice). Pick the smallest host-testable one. Run `gh issue list --state open` and read each issue before choosing.
- **Session-delete follow-on (only if the owner wants it):** now that every turn-child table cascades, a `SessionStore::delete_session` + GUI "delete this session" feature is unblocked. This is new feature work, so brainstorm it with the owner first.

### 3. Production-code splits (owner-approved lane, all feature-gated, so dual-verify)
- **`primer-inference/src/qnn/genie/real.rs`** (566 lines, `qnn`) is the recommended one. Baselines:
  - `cargo test -p primer-inference`;
  - `cargo clippy -p primer-inference --features qnn --all-targets -- -D warnings`;
  - the rustdoc `qnn` arm.
- **`primer-speech/src/macos/{tts.rs 668, stt.rs 504}`** (`macos-native`).
- **Low value:** `state_machine/inner.rs` (506).
- **Hardest:** `primer-cli/src/main.rs` (1353).

### Carried / owner-or-hardware-gated
- **#260:** Android-voice on-device acceptance. Also re-check, on device, that a denied mic now keeps the text session (#369).
- **#192:** manual macOS-native STT + injected TTS smoke.
- **#170 Stages B, E and F:** the OpenRAIL-M disclosure must ship before any default flip.
- **#166 item #1:** owner-run WhisperStream reuse smoke.
- **#135:** glib (blocked on Tauri 3).
- **Branch protection:** make `cargo test (default features)` a required check.
- **Other:** QNN token stability, NPU tuning, latency-routing calibration.

## Open decisions / risks

### #352 item 2: what counts as a pointless fallback? (owner decision, unchanged)
`--fallback-backend == --backend` with a different **model** is a legitimate setup: small/big ollama models, or hybrid routing on one server.

**Options:**
- (a) notice only when kind **and** model match;
- (b) kind-only, but worded as "no protection if the server goes down";
- (c) drop the item and keep the `tracing::warn!`.

**Recommendation:** (a). Also downgrade or drop today's kind-only warn on the router path.

### #369
- **A session-build failure after the close still loses the text session.** Marked with a `shortcut:` comment in `start.rs`. It is rare (it needs a config change between the two builds), but it is not zero.
- **Backends are now built while a voice loop could still be running.** The frontend guards with `state.active` / `state.starting`, so this is only the defensive step-2 path. If it ever happens, a second cpal mic would open briefly. On hosts that refuse a second stream, the start now fails and leaves the old loop running, which is safer than before.
- **Not checked in a live `cargo tauri dev` session** or on an Android device.
- **New dev-only dependency surface:** `tauri`'s `test` feature (`MockRuntime`).

### #370
- **v9 rebuilds the table on first open.** That is a full copy, but `turn_classifications` holds one row per turn, so it is small.
- **The drop / rename runs with `foreign_keys = ON`.** This is safe because no table references `turn_classifications`. **If a future table ever FKs to it, revisit this.**

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
- **Driving a real `#[tauri::command]` in a test:** make it generic over `R: tauri::Runtime`, then call it with `tauri::test::mock_app()` + `app.manage(AppState::new(..))` + `app.state()`. See `commands/voice/tests.rs::app_with_text_session`.
- **Migration that changes an existing FK:** follow v9 (`pragma_foreign_key_list` guard + a create / copy / drop / rename tx).
- **Session start:** check `git status` first, and read `NEXT_SESSION.md` AFTER `git pull`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status --short && git checkout main && git pull --ff-only && git log --oneline -6 && gh pr list --state open
# Open at close: #369, #370. After merge: gh issue view 366 --json state; gh issue view 331 --json state   (352 stays open)
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

- **#369:** if starting voice mode fails (for example, you cancel the voice-model download, or the microphone isn't allowed), your text conversation now keeps working instead of silently ending. Closes #366.
- **#370:** the session database can now delete a whole conversation cleanly. Before, the stored engagement labels blocked it. This unblocks a future "delete my history" feature. Closes #331.
