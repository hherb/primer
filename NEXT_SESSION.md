# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-10-22 (UTC). **One PR is open against `main`:**

| PR | Branch | What | Closes |
|---|---|---|---|
| **#359** | `fix/gui-notice-wording-341` | Engine errors name both the CLI flag and the GUI Settings field. | #341 |

`main` is at `541f5f7` (#357). Since the last brief, **#357 merged** and **#355 is confirmed CLOSED**.

## What we shipped this session

### #359 (`d88f786`): closes #341
**Problem:** `primer-engine` construction errors are shown verbatim in both frontends: on CLI stderr, and in the GUI's inline error and `BackendNotice` banner. The hints named only CLI flags and `primer-cli/<feature>` rebuild commands, which mean nothing to a GUI user.

**Fix (pure string changes in `primer-engine/src/wiring/{backend,embedders,subsystems}.rs`):**
- Every actionable message now names both the flag and the Settings path, e.g. "`--fallback-model`, or Settings → Inference backend → Fallback model".
- Feature hints name both `primer-cli/<f>` and `primer-gui/<f>`.
- Messages covered: cloud key, fallback model, QNN bundle dir, llamacpp GGUF path, the three subsystem models, the openai-compat embedder model, and the five feature hints.
- The same-backend fallback `warn!` is now worded neutrally.

**Design choice:** one dual-naming string, not a per-frontend hint threaded through `BackendParams`. As a result, **CLI messages are slightly longer** (the earlier acceptance criterion said "CLI wording unchanged"; the flag is still named, so the CLI user can still act). Flag this to the owner if they want byte-identical CLI text.

**TDD:**
- New `wiring/error_wording_tests.rs`: 8 default-build tests plus one `openai-compat-embedding` arm.
- All were red before the fix.
- Helpers `assert_names_both(msg, flag, settings_path)` and `assert_names_both_crates(msg, feature)`.

**CI:** the `cargo test -p primer-engine --features ollama-embedding,openai-compat-embedding` step is now **unfiltered** (it was filtered to `embedder_fallback`), so the gated arm runs.

**Docs:** the CLAUDE.md `BackendNotice` sentence now describes the dual wording. README and ROADMAP needed nothing.

**Gate results:**
- `cargo test --workspace`: 1357/0.
- Engine with the two embedding features: 92/0.
- clippy `-D warnings`: clean for the workspace, and for `primer-engine` with `qnn,embedding,ollama-embedding,openai-compat-embedding` and with `llamacpp`.
- fmt: clean.
- `primer-engine` rustdoc: clean.
- **Not run:** the other rustdoc feature arms (no doc comments changed) and a GUI visual check of the longer banner text.

**Left as is:**
- the CLI-only `--resume` locale message (`learner.rs`), since the GUI never reaches it;
- the log-only subsystem "model ignored: backend is stub" warns (still flag wording, log-only).

### Per-session sweep
- The oversized list is unchanged.
- The **inline-test detector is clean for the twelfth session running**.
- Watch-list: `primer-classifier/src/llm.rs` (441), `primer-extractor/src/llm.rs` (454), `primer-engine/src/wiring/backend.rs` (now ~450 after #359).

## What's next (concrete acceptance criteria)

### 0. Owner review/merge: #359
- **Acceptance:** CI is green and the owner merges.
- Then confirm #341 auto-closed: `gh issue view 341 --json state`.

### 1. Sweep + inline-test detector: re-run EVERY session
Commands are in the resume block.

### 2. Remaining review-filed bugs (host-completable; TDD each: failing test, fix, `Closes #N` PR)
- **#340:** stage-test gaps. This is the recommended next one; start with `gh issue view 340`.
- **#351:** subsystem ids still name the primary under `PrimaryUnavailable`.
- Older ones: #307–#316 (retrieval/storage/streaming), #325/#326 (voice), #331/#332 (storage).

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
- **Notice text is English-only** and operator-facing (`BackendNotice`, `retrieval_notice`).
  - Since #359 it names both the CLI flag and the Settings path.
  - It is still not localised, and `Other`'s dev string still passes through.
- **#359's Settings paths are hand-written strings** ("Settings → Inference backend → Fallback model").
  - If a GUI label in `ui/index.html` is renamed, the engine text drifts silently; no test ties the two together.
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
- **Messages shared by the CLI and GUI → name both forms in one string** (flag + Settings path; `primer-cli/<f>` + `primer-gui/<f>`) and pin them with `assert_names_both` in `primer-engine/src/wiring/error_wording_tests.rs`. Prefer this to threading a frontend enum through `BackendParams`.
- **Two issues needing the same plumbing → one PR.** One typed return value can fix both (#347 + #338).
- **Changing a widely-called signature:** mechanically rewrite test call sites with `perl -pi`, so the red phase is pure compile errors.
- **Duplicated GUI+CLI logic → one pure helper in `primer-engine`** that takes the *result* of the I/O call (`adopt_or_mint_learner_id`).
- **Split recipe, substantive-line diff, Dependabot triage:** unchanged; see `docs/handoffs/2026-10-16T202752Z.md` § "Patterns to reuse".
- **Stacked PRs:** after the base squash-merges, retarget or rebase the child **before** merging it (the #344 loss).
- Read `NEXT_SESSION.md` AFTER `git pull`, and check `git status` first.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status --short && git checkout main && git pull --ff-only && git log --oneline -6 && gh pr list --state open
# Open at close: #359. After merge, confirm #341 closed: gh issue view 341 --json state
S=/private/tmp/primer-scratch; mkdir -p "$S"   # or the session's scratchpad dir

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done

# === Next bug (#340): start here ===
gh issue view 340
git checkout -b fix/stage-test-gaps-340 origin/main
~/.cargo/bin/cargo test -p primer-engine -p primer-gui 2>&1 | grep 'test result'   # baseline

# === Standard workspace gate (one cargo pass at a time) ===
~/.cargo/bin/cargo test --workspace 2>&1 | tee $S/ws.log | tail -3
grep -E 'test result: FAILED|^error' $S/ws.log  # expect empty
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo fmt --all -- --check
~/.cargo/bin/cargo test -p primer-engine --features ollama-embedding,openai-compat-embedding   # unfiltered since #359

# === Rustdoc gate (CI-enforced); feature arms mirror ci.yml ===
RUSTDOCFLAGS="-D warnings" ~/.cargo/bin/cargo doc --workspace --no-deps --document-private-items
for spec in "-p primer-speech --features voice-loop,silero,whisper,piper,cpal,supertonic" "-p primer-speech --features android-native" "-p primer-speech --features voice-loop,android-native" \
  "-p primer-inference --features qnn" "-p primer-inference --features llamacpp" "-p primer-gui --features speech" "-p primer-gui --features android-native" \
  "-p primer-cli --features speech" "-p primer-speech --features voice-loop,silero,cpal,macos-native" "-p primer-speech --features voice-loop,silero,cpal,macos-native-26"; do \
  RUSTDOCFLAGS="-D warnings" ~/.cargo/bin/cargo doc ${=spec} --no-deps --document-private-items >/dev/null 2>&1; echo "$spec exit=$?"; done
```

## Reporting back

- **#359:** engine errors now tell the user what to change in either frontend. They name both the CLI flag and the Settings field, and rebuild hints name both crates.
  - It closes #341 with red-then-green tests.
  - Workspace gate is green: 1357/0.
- #357 merged; #355 is closed.
