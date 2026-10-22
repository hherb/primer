# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-10-22 (UTC). **One PR is open against `main`:**

| PR | Branch | What | Closes |
|---|---|---|---|
| **#357** | `fix/gui-tracing-sink-355` | Desktop GUI tees `tracing` into `~/.primer/logs/gui.log`. | #355 |

`main` is at `cfd1da4` (#356). Since the last brief, **#356 merged** and **#353 is confirmed CLOSED**.

## What we shipped this session

### #357 (`50774e6`): closes #355
**Problem:** a Dock/Finder launch sends stdout to `/dev/null`, and the desktop `init_tracing` arm wrote only to stdout. Every GUI `warn!` was lost, including the #336 embedder fallback.

**GUI (`primer-gui`):**
- New pure, desktop-only module `src/log_sink/` (`mod.rs` plus `tests.rs`):
  - `desktop_log_path(home)` returns `~/.primer/logs/gui.log`.
  - `rotate_if_oversized(log, max)` moves the log to `gui.log.1` (one generation) once it reaches `GUI_LOG_MAX_BYTES` (5 MiB).
  - `open_log_file` creates the directory, rotates, then opens for append.
  - `desktop_subscriber(filter, Option<File>)` combines a stdout layer with an ANSI-free file layer.
- `lib.rs::init_tracing` desktop arm uses it. If the file won't open, the app reports that on stderr and keeps logging to stdout.
- Android's logcat arm is unchanged. **No new dependency** (`tracing-appender` was not added).

**TDD:**
- Red: compile error against `todo!()` stubs. Green: 7 new tests.
- Key test: `desktop_subscriber_tees_events_into_the_log_file` (a `warn!` is written, a filtered `debug!` is not, and there are no ANSI escapes).

**Docs:** CLAUDE.md, `docs/devel/04-knowledge-and-retrieval.md`, README (GUI section), and the `primer-engine` `embedders.rs` doc comment. ROADMAP needed nothing.

**Gate results:**
- `cargo test -p primer-gui`: 232/0.
- clippy `-D warnings`: clean for `primer-gui` default, `speech` and `android-native` (host).
- fmt: clean.
- rustdoc `-D warnings`: clean for `primer-gui` and `primer-engine`.
- **Not run:** the full `cargo test --workspace` (only `primer-gui` code changed), the rustdoc `speech`/`android-native` arms after the final link fix (the earlier run failed only on that link), and a real Android target build.

### Per-session sweep
- The oversized list is unchanged.
- The **inline-test detector is clean for the eleventh session running**.
- Watch-list: `primer-classifier/src/llm.rs` (441), `primer-extractor/src/llm.rs` (454), `primer-engine/src/wiring/backend.rs` (443).

## What's next (concrete acceptance criteria)

### 0. Owner review/merge: #357
- **Acceptance:** CI is green and the owner merges.
- Then confirm #355 auto-closed: `gh issue view 355 --json state`.

### 1. Sweep + inline-test detector: re-run EVERY session
Commands are in the resume block.

### 2. Remaining review-filed bugs (host-completable; TDD each: failing test, fix, `Closes #N` PR)
- **#341:** CLI-flag wording in engine errors reaches GUI users. This also covers `BackendNotice.reason`.
  - **Acceptance:** GUI-facing notice and error text names Settings fields, not `--flags`, while CLI wording is unchanged. A test pins both renderings.
- **#340:** stage-test gaps.
- Older ones: #307–#316 (retrieval/storage/streaming), #325/#326 (voice), #331/#332 (storage), #351 (subsystem ids under `PrimaryUnavailable`).

### 3. Production-code splits (owner-approved lane, all feature-gated, so dual-verify)
- **`primer-inference/src/qnn/genie/real.rs` (566, `qnn`)** is the recommended one. Baselines:
  - `cargo test -p primer-inference`;
  - `cargo clippy -p primer-inference --features qnn --all-targets -- -D warnings`;
  - the rustdoc `qnn` arm.
- `primer-speech/src/macos/{tts.rs 668, stt.rs 504}` (`macos-native`; this host is macOS).
- Low value: `state_machine/inner.rs` (506). Hardest: `primer-cli/src/main.rs` (~1350, heavily cfg-gated).
- **The gate for every split includes the rustdoc arms.** An outer `///` plus an inner `//!` on the same module breaks links (CLAUDE.md rule 3).

### Carried / owner-or-hardware-gated
- **#260**: Android-voice on-device acceptance.
- **#192**: manual macOS-native STT + injected TTS smoke.
- **#170 Stages B, E and F**: the OpenRAIL-M disclosure must ship before any default flip.
- **#166 item #1**: owner-run WhisperStream reuse smoke.
- **#135**: glib (blocked on Tauri 3).
- **Branch protection**: make `cargo test (default features)` a required check.
- QNN token stability, NPU tuning, latency-routing calibration.

## Open decisions / risks

- **GUI log rotation runs only at startup** (marked `shortcut:` in `log_sink/mod.rs`).
  - One very long session can grow `gui.log` past 5 MiB.
  - Upgrade to a size-checking writer only if that shows up in practice.
- **The log file may hold child text.**
  - Info-level events can echo session content.
  - It sits in `~/.primer/` beside the session DBs (same local-only posture), but a parent sharing the log for support is sharing that content.
- **Not seen live:**
  - no Dock launch of a bundled `.app` has been run;
  - the #353 banner hasn't been checked in `cargo tauri dev`.
- **The #353 failed-embedder case is pinned only via the pure helper.** The end-to-end test covers `none`/`stub`; fastembed failure needs network control.
- **Notice text is English-only** and operator-facing (`BackendNotice`, `retrieval_notice`). Wording cleanup is #341.
- **Stale CMake cache after Xcode/SDK upgrades.** Symptom: `'<header>' file not found` from a `*-sys` build. Recipe:
  1. Find the stale caches: `find src/target -name CMakeCache.txt -maxdepth 6 | xargs grep -l 'MacOSX<old>'`.
  2. `cargo clean -p <those sys crates>`.
- **The `github` MCP server fails to connect** (bad Authorization header). The `gh` CLI works.
- **zsh gotchas:**
  - `$var` is not word-split; use `${=spec}`.
  - An unquoted `echo ===X` triggers `=` expansion; quote it.
  - `grep --include=*.rs` globs fail; use `git grep … -- '*.rs'`.

## Patterns to reuse, not reinvent

- **A desktop-only GUI module** (`#[cfg(not(target_os = "android"))] pub mod …` in `lib.rs`) is still compiled by the host `android-native` clippy arm, since that arm only enables the feature and does not change the target. Only a real `--target aarch64-linux-android` build drops it.
- **The rustdoc gate catches public docs that link to a private fn** (`rustdoc::private_intra_doc_links`), even with `--document-private-items`. Describe the private item in prose instead.
- **Bug-fix recipe:**
  1. Branch off `origin/main`.
  2. Write the failing test, confirm red, fix, confirm green.
  3. Run test + clippy for each touched crate (plus the feature arms whose files you touched), then fmt.
  4. Hunt for doc drift: `git grep` the behaviour across `*.md`.
  5. Commit with `Closes #N`, push, open the PR.
  6. File any out-of-scope half as a new issue before writing the PR body.
- **Testing that something is logged:** install a thread-local `tracing_subscriber::fmt()` with a `MakeWriter` over `Arc<Mutex<Vec<u8>>>`, via `tracing::subscriber::set_default`. `#[tokio::test]` is current-thread, so awaited events are captured. See `primer-engine/src/wiring/embedder_fallback_tests.rs`.
- **Feature-gated tests:** gate the whole test module on the features, not each test, so the shared helpers aren't dead code on a default build. Then add a CI step, or no job will ever run them.
- **Unreachable endpoint for network-failure tests:** `http://127.0.0.1:1` refuses fast, with no mock server needed.
- **A new UI notice = derive it in GUI wiring from data already in hand** (here, `config.embedder.kind` + `subsystems.embedder.is_some()`), not by changing an engine return type. Then forward it at every `SessionInfo {` site (`git grep -n 'SessionInfo {'`) and clippy the `speech` and `android-native` arms, which own two of those sites.
- **Two issues needing the same plumbing → one PR.** One typed return value can fix both (#347 + #338).
- **Changing a widely-called signature:** mechanically rewrite test call sites with `perl -pi`, so the red phase is pure compile errors.
- **Duplicated GUI+CLI logic → one pure helper in `primer-engine`** that takes the *result* of the I/O call (`adopt_or_mint_learner_id`).
- **Split recipe, substantive-line diff, Dependabot triage:** unchanged; see `docs/handoffs/2026-10-16T202752Z.md` § "Patterns to reuse".
- **Stacked PRs:** after the base squash-merges, retarget or rebase the child **before** merging it (the #344 loss).
- Read `NEXT_SESSION.md` AFTER `git pull`, and check `git status` first.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status --short && git checkout main && git pull --ff-only && git log --oneline -6 && gh pr list --state open
# Open at close: #357. After merge, confirm #355 closed: gh issue view 355 --json state
S=/private/tmp/primer-scratch; mkdir -p "$S"   # or the session's scratchpad dir

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done

# === Next bug (#341): start here ===
gh issue view 341
git checkout -b fix/gui-notice-wording-341 origin/main
~/.cargo/bin/cargo test -p primer-engine -p primer-gui 2>&1 | grep 'test result'   # baseline

# === Standard workspace gate (one cargo pass at a time) ===
~/.cargo/bin/cargo test --workspace 2>&1 | tee $S/ws.log | tail -3
grep -E 'test result: FAILED|^error' $S/ws.log  # expect empty
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo fmt --all -- --check
~/.cargo/bin/cargo test -p primer-engine --features ollama-embedding,openai-compat-embedding embedder_fallback   # new in #354

# === Rustdoc gate (CI-enforced); feature arms mirror ci.yml ===
RUSTDOCFLAGS="-D warnings" ~/.cargo/bin/cargo doc --workspace --no-deps --document-private-items
for spec in "-p primer-speech --features voice-loop,silero,whisper,piper,cpal,supertonic" "-p primer-speech --features android-native" "-p primer-speech --features voice-loop,android-native" \
  "-p primer-inference --features qnn" "-p primer-inference --features llamacpp" "-p primer-gui --features speech" "-p primer-gui --features android-native" \
  "-p primer-cli --features speech" "-p primer-speech --features voice-loop,silero,cpal,macos-native" "-p primer-speech --features voice-loop,silero,cpal,macos-native-26"; do \
  RUSTDOCFLAGS="-D warnings" ~/.cargo/bin/cargo doc ${=spec} --no-deps --document-private-items >/dev/null 2>&1; echo "$spec exit=$?"; done
```

## Reporting back

- **#357:** the desktop app now writes its diagnostic log to `~/.primer/logs/gui.log`, so warnings survive a Dock/Finder launch.
  - The log is rotated at 5 MiB.
  - It closes #355 with red-then-green tests; the `primer-gui` gate is green (232 tests).
- #356 merged; #353 is closed.
