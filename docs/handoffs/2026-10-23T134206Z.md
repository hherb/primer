# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-10-23 (UTC).

**One PR is open against `main`:**

| PR | Branch | What | Closes |
|---|---|---|---|
| **#363** | `fix/subsystem-ids-degraded-351` | Subsystems follow the served backend under `PrimaryUnavailable` (this brief rides on it) | #351 |

`main` is at `0570967` (#361).

**Since the last brief:**
- **#360 and #361 merged.**
- **#358 and #340 are confirmed CLOSED.**

## What we shipped this session

### #363 (`0dd2ef0`): closes #351

**Problem:** under `BackendNotice::PrimaryUnavailable`, the fallback serves every turn, but the classifier, extractor and comprehension builders still got the **primary's** kind and model. That caused two bugs:
1. **Wrong provenance ids.** Inherited subsystems ran on the fallback but were labelled after the primary:
   - `llm:<primary model>`;
   - `llm:cloud-anthropic` for a qnn or llamacpp primary.
2. **Aborted startup.** A model-only override rebuilt the unavailable primary kind and aborted startup:
   - CLI: it exited;
   - GUI: it failed with "constructing engagement classifier".

**Fix:**
- `served_identity` moved from `primer-gui/src/wiring/backend_params.rs` to `primer-engine/src/wiring/backend.rs` and is re-exported as `primer_engine::served_identity`.
- Both binaries pass its `(kind, model)` to the subsystem builders.
- GUI `build_subsystems` takes `backend_kind` explicitly.
- An explicit `--*-backend` override is unchanged: it still builds exactly the kind asked for.

**Tests:**
- New GUI tests in `primer-gui/src/wiring/tests.rs`, both mutation-checked red against the old call site:
  - `primary_unavailable_subsystems_name_served_fallback`
  - `primary_unavailable_override_subsystem_builds_on_fallback`
- The `served_identity` unit tests moved to `primer-engine/src/wiring/build_main_backend_tests.rs`.
- The CLI was smoke-run on the issue's override scenario. It now prints `Engagement classifier: llm:override-x`; before the fix it exited.

**Docs:** the CLAUDE.md `FallbackBackend` paragraph is updated. The "Sharper edge" text and the "#351 known mislabel" note are replaced.

**Gate:**
- `cargo test --workspace`: 1375/0.
- clippy `-D warnings`: clean for the workspace, `primer-gui` (`speech`, `android-native`) and `primer-cli` `speech`.
- fmt: clean.
- rustdoc for engine, gui and cli: clean.

**Filed rather than fixed:** the issue's "ollama variant" was added to **#352 as item 4**. An overridden subsystem pointing at a dead server soft-fails every turn, and that is only logged. It is a runtime signal, not a construction problem.

### Per-session sweep
- **Inline-test detector:** clean for the fourteenth session running.
- **Oversized list:** unchanged, except that `primer-gui/src/wiring/stage_tests.rs` went to 515 after the #360/#361 merge and is back to 462 on #363, because the `served_identity` tests moved out.

## What's next (concrete acceptance criteria)

### 0. Owner review/merge: #363
- **Acceptance:** CI is green and it merges.
- Then check the issue is closed: `gh issue view 351 --json state`.

### 1. Sweep + inline-test detector: re-run EVERY session
Commands are in the resume block.

### 2. Remaining review-filed bugs (host-completable; TDD each: failing test, fix, `Closes #N` PR)
- **#352** (recommended next): it now has four items. Take them in order of size:
  - **Item 3** (smallest): the GUI header and `#backend-notice-banner` stay up after `stop_voice_mode`.
    - **Acceptance:** after a voice stop, `resetSessionHeader` (or an equivalent refresh) clears both.
    - This is JS-only, in `ui/voice.js`.
  - **Item 2:** report a same-kind fallback (`--fallback-backend == --backend`) as a third `BackendNotice` variant, on both the fallback path and the router path.
    - **Acceptance:** `build_main_backend` returns the new notice in both modes. Extend `build_main_backend_tests.rs`.
  - **Items 1 and 4:** per-turn "served by" and subsystem soft-fail signals. These are larger and need a design note first.
- **#331 / #332** (storage): cascade delete for `turn_classifications`; shape assertions for the v8 migration. Small and well-scoped.
- **Older ones:** #307–#316 (retrieval/storage/streaming), #325/#326 (voice).

### 3. Production-code splits (owner-approved lane, all feature-gated, so dual-verify)
- **`primer-inference/src/qnn/genie/real.rs`** (566 lines, `qnn`) is the recommended one. Baselines:
  - `cargo test -p primer-inference`;
  - `cargo clippy -p primer-inference --features qnn --all-targets -- -D warnings`;
  - the rustdoc `qnn` arm.
- **`primer-speech/src/macos/{tts.rs 668, stt.rs 504}`** (`macos-native`).
- **Low value:** `state_machine/inner.rs` (506).
- **Hardest:** `primer-cli/src/main.rs` (~1350).

### Carried / owner-or-hardware-gated
- **#260:** Android-voice on-device acceptance.
- **#192:** manual macOS-native STT + injected TTS smoke.
- **#170 Stages B, E and F:** the OpenRAIL-M disclosure must ship before any default flip.
- **#166 item #1:** owner-run WhisperStream reuse smoke.
- **#135:** glib (blocked on Tauri 3).
- **Branch protection:** make `cargo test (default features)` a required check.
- **Other:** QNN token stability, NPU tuning, latency-routing calibration.

## Open decisions / risks

### #363
- **Under `PrimaryUnavailable`, a model-only subsystem override now runs on the fallback kind** (for example, a cloud fallback with `--classifier-model override-x` sends `override-x` to Anthropic).
  - This is the issue's suggested fix, and `--fallback-backend` is the consent to use that kind.
  - However, the override model name was originally chosen for the *primary* kind. If it isn't valid on the fallback, the subsystem soft-fails per turn, which is only logged (#352 item 4).
- **The CLI's "Using X backend" banner still names the configured primary.** The `Warning:` line just above it explains the switch to the fallback.
- **Existing rows written under the mislabel are not rewritten.** Rows in `turn_classifications` / `comprehension_classifiers` from earlier degraded sessions keep the wrong id. No migration is planned, because there's no way to tell which rows were affected.

### Carried from earlier sessions
- **#360 permissions:**
  - Existing `0644` DB files are protected only by the `0700` parent directory.
  - The tighten step runs only when a session opens.
  - Windows and Android are untouched.
- **GUI logging:** log rotation runs only at startup (`shortcut:` in `log_sink/mod.rs`), and the log file may hold child text.
- **Not seen live:** no Dock launch of a bundled `.app`; the #353 banner hasn't been checked in `cargo tauri dev`.
- **Notice text is English-only** and operator-facing. #359's Settings paths are hand-written strings, so they can drift silently if a GUI label is renamed.
- **Stale CMake cache after Xcode/SDK upgrades:**
  1. `find src/target -name CMakeCache.txt -maxdepth 6 | xargs grep -l 'MacOSX<old>'`
  2. `cargo clean -p <those sys crates>`
- **The `github` MCP server fails to connect** (bad Authorization header). The `gh` CLI works.
- **zsh gotchas:**
  - `$var` is not word-split; use `${=spec}`.
  - Quote `echo ===X`.
  - Use `git grep … -- '*.rs'`, not `--include` globs.

## Patterns to reuse, not reinvent

- **"What serves turns" is one function.** Use `primer_engine::served_identity(kind, model, notice, backend.name())` anywhere that needs the effective backend: sidebar, subsystem ids, logs. Never re-derive it from `config.backend.kind`.
- **End-to-end degraded-primary tests in the GUI:**
  - Use `kind = "qnn"`; it always fails on a host build.
  - Pair it with a `cloud` fallback and an **Inline** key, so the test never depends on the shell.
  - See `primary_unavailable_cloud_fallback_config()` in `primer-gui/src/wiring/tests.rs`.
- **Id formats differ per subsystem:**
  - the classifier is `llm:<model>`;
  - the extractor and comprehension are `llm:<backend.name()>:<model>`.

  Check the format before writing the assertion.
- **Testing env-var-dependent code:**
  - Inject the lookup as `impl Fn(&str) -> Option<String>`.
  - In tests, pass a recording closure.
  - Never use `set_var`.
  - See `recording_env` in `stage_tests.rs`.
- **Proving `Arc` reuse:** hand in a backend whose `name()` differs from what a fresh build would produce.
- **Mutation-check new tests:**
  1. Back up the file to the scratchpad.
  2. Apply the mutation with `perl -0pi`.
  3. Run the filtered tests.
  4. Restore the file.
- **Python `sub()` edits:** a helper that asserts the match stops before the write. Chain the next git step with `&&`.
- **Owner-only dirs on Unix:** use `primer_engine::create_private_dir_all`.
- **Bug-fix recipe:**
  1. Branch off `origin/main`.
  2. Failing test, then fix, then green.
  3. Test + clippy each touched crate, plus its feature arms. Then fmt.
  4. `git grep` the docs for drift.
  5. Commit with `Closes #N`, push, open the PR.
  6. File any out-of-scope half as an issue. Add it to an existing issue if one covers the same family.
- **CLI/GUI-shared messages:** name both forms in one string, pinned by `assert_names_both`.
- **Split recipe, substantive-line diff, Dependabot triage:** see `docs/handoffs/2026-10-16T202752Z.md`.
- **Session start:** check `git status` first, and read `NEXT_SESSION.md` AFTER `git pull`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status --short && git checkout main && git pull --ff-only && git log --oneline -6 && gh pr list --state open
# Open at close: #363. After merge: gh issue view 351 --json state
S=/private/tmp/primer-scratch; mkdir -p "$S"   # or the session's scratchpad dir

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done

# === Next bug (#352, item 3 first): start here ===
gh issue view 352 --comments
git checkout -b fix/voice-stop-header-352 origin/main
grep -n "setActive\|resetSessionHeader" crates/primer-gui/ui/*.js

# === Standard workspace gate (one cargo pass at a time) ===
~/.cargo/bin/cargo test --workspace 2>&1 | tee $S/ws.log | tail -3
grep -E 'test result: FAILED|^error' $S/ws.log  # expect empty
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo fmt --all -- --check
~/.cargo/bin/cargo test -p primer-engine --features ollama-embedding,openai-compat-embedding

# === Rustdoc gate (CI-enforced); feature arms mirror ci.yml ===
RUSTDOCFLAGS="-D warnings" ~/.cargo/bin/cargo doc --workspace --no-deps --document-private-items
for spec in "-p primer-speech --features voice-loop,silero,whisper,piper,cpal,supertonic" "-p primer-speech --features android-native" "-p primer-speech --features voice-loop,android-native" \
  "-p primer-inference --features qnn" "-p primer-inference --features llamacpp" "-p primer-gui --features speech" "-p primer-gui --features android-native" \
  "-p primer-cli --features speech" "-p primer-speech --features voice-loop,silero,cpal,macos-native" "-p primer-speech --features voice-loop,silero,cpal,macos-native-26"; do \
  RUSTDOCFLAGS="-D warnings" ~/.cargo/bin/cargo doc ${=spec} --no-deps --document-private-items >/dev/null 2>&1; echo "$spec exit=$?"; done
```

## Reporting back

- **#363:** when the main backend is down at startup and the fallback takes over, the background classifiers now run on the fallback and are labelled after it. A per-classifier model setting no longer crashes startup. Closes #351.
- The silent-runtime half of the issue is now item 4 on #352.
- Workspace gate: 1375/0.
