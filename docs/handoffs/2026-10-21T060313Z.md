# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-10-21 06:05 UTC. **Four PRs are open against `main`, all independent:**

| PR | Branch | What | Closes |
|---|---|---|---|
| **#345** | `docs/reland-rustdoc-gate` | Re-land of #344 (rustdoc fixes + `cargo doc` CI gate); also carries this brief | #330, #319 |
| **#346** | `fix/learner-id-adoption-339` | A learner-id lookup error now propagates instead of minting and persisting a UUID | #339 |
| **#348** | `fix/gui-env-files-337` | The GUI loads `.env` and `~/.primer_env` at startup, like the CLI | #337 |
| **#349** | `fix/match-main-model-335` | "Match main" also ignores a stale subsystem `model` override | #335 |

`main` is at `252b224` (#343, rustls). Since the last brief, #342 and #343 merged. **#344 did NOT land on `main`** (see below).

## What we shipped this session

### #344 was lost to the stacked-PR trap, so it is re-landed as #345
- **What happened:** #342 squash-merged to `main` at 02:50:35. #344, stacked on #342, merged **52 s later into the now-stale `refactor/split-gui-voice-commands` branch**. GitHub had not yet retargeted it to `main`. None of #344's rustdoc fixes or CI gate reached `main`, and #330 and #319 stayed open.
- **Fix:** I cherry-picked its three commits unchanged onto `origin/main`:
  - `fa2d24c`: link fixes + CI gate;
  - `70bfe2a`: CLAUDE.md citations + the old brief;
  - `fcb7d75`: review follow-up (the cpal-free android-native doc arm, stale docs, `[[memory]]` refs).
- **Verified:** the doc arms re-run clean on the new base, and fmt is clean.
- **#345's CI ran the `cargo doc` steps on Linux for the first time, and they passed** in the `test` job, `feature-combos` and macOS. The "unproven on Linux" risk from the last brief is retired.
- CLAUDE.md's gate entry now cites #345.
- **Earlier in this session (before the brief), I reviewed #344 and applied all the findings.** The notable one: the cpal-free `primer-speech --features android-native` config (what the APK ships) failed rustdoc on a cpal-gated link, and CI never documented that config. Both are fixed, and CI now documents android-native bare **and** with `voice-loop`.

### #346 (`1b7f71b`): closes #339
- New pure helper `primer_engine::adopt_or_mint_learner_id(Result<Option<Uuid>>) -> Result<Uuid>`:
  - `Ok(Some)` → adopt;
  - `Ok(None)` → mint;
  - `Err` → **propagate**.
- The GUI (`wiring/stores.rs`) and CLI (`main.rs`) replace their duplicated `match` with it, adding error context and `?`.
- TDD: 3 unit tests in `primer-engine/src/learner.rs`, red then green.
- Tests: engine 75, CLI 23, GUI 217; clippy and fmt clean. CI green apart from the CodeQL rust scan, which was still pending.

### #348 (`b32059e`): closes #337, env-file half
- New `primer_engine::env_files`:
  - `load_env_files()`: `.env`, then `$HOME/.primer_env`; earlier sources win;
  - `load_user_env_file(home) -> bool`;
  - `USER_ENV_FILE`.
- Both binaries call it. `primer-cli` drops its direct `dotenvy` dep.
- The GUI calls it first in its desktop setup block, before any thread spawns (`set_var` safety). Mobile is untouched.
- A malformed `~/.primer_env` is now logged, and a non-UTF-8 API-key env value warns via `env_key`.
- Docs: CLAUDE.md, README and `docs/devel/01-getting-started.md`.
- TDD: 2 tests in `env_files::tests`.
- **The banner half is filed as #347:** report `build_main_backend`'s `PrimaryAlone` outcome to the GUI.

### #349 (`64dac6a`): closes #335
- New `subsystem_model()` mirrors `subsystem_kind()`, and the three `*_model` `BackendParams` fields use it.
- The `SubsystemConfig` doc now says `timeout_ms` still applies under match-main. That is intentional; it's a per-subsystem tunable.
- TDD: the existing stage test, extended with stale models, went red (`Some("stale-model")`) then green. GUI tests: 217.

### Per-session sweep
- The oversized list is unchanged from last session.
- The **inline-test detector is clean for the seventh session running**.
- Watch-list: `primer-classifier/src/llm.rs` (441), `primer-extractor/src/llm.rs` (454).

## What's next (concrete acceptance criteria)

### 0. Owner review/merge: #345, #346, #348, #349
- All four are independent and branch from `main`.
- **Expect merge conflicts:**
  - #345 and #348 both touch `CLAUDE.md`, in different hunks: line ~41 (env files) and the `cargo doc` / `primer-speech` bullets. They should auto-merge.
  - #346 and #348 both touch `primer-engine/src/lib.rs` (the `pub use learner::{…}` list vs. a `pub mod env_files;` line) and `primer-cli/src/main.rs` (the imports vs. the env block). Adjacent edits, so a trivial rebase may be needed for whichever merges second.
- **Acceptance:** CI green and the owner merges. After #345 merges, confirm on `main` that #330 and #319 auto-closed; close them by hand if not.
- **Lesson, now in the habits list:** never merge a stacked PR within a minute of its base PR. Retarget it first or rebase it.

### 1. Sweep + inline-test detector: re-run EVERY session
Commands are in the resume block.

### 2. Remaining review-filed bugs (all host-completable; TDD each: failing test, fix, `Closes #N` PR)
- **#347** (new): surface a dropped cloud fallback to the GUI user.
  - Have `build_main_backend` return its plan outcome, then show a banner.
  - Pure-test the plan → message mapping.
- **#338:** the sidebar shows the local primary while the cloud fallback serves turns.
- **#336:** use `tracing::warn!` instead of `eprintln!` for embedder fallback notices.
- **#341:** CLI-flag wording in engine errors reaches GUI users.
- **#340:** stage-test gaps.
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

- **#345 keeps the façade `allow` choice** over #330's "escape the links" suggestion. That is the owner's call; flagged in #344/#345.
- **#346 changes startup behaviour.** A DB error while reading the most recent session's learner id is now a hard startup error (GUI session-start error; CLI exits), not a silent identity mint. That is the intended trade, since the old path silently corrupted data. It is the same posture as an existing `load_learner` failure.
- **#348 loads `.env` from the GUI's working directory.** For `cargo tauri dev` that is the repo's `.env`; for a Dock launch it is `/`, which is harmless. Not exercised: a real Dock launch of the bundled `.app`.
- **Stale CMake cache after Xcode/SDK upgrades.** Symptom: `'<header>' file not found` from a `*-sys` build. Recipe:
  1. Find the stale caches: `find src/target -name CMakeCache.txt -maxdepth 6 | xargs grep -l 'MacOSX<old>'`.
  2. `cargo clean -p <those sys crates>`.
- **The `github` MCP server fails to connect** (bad Authorization header). The `gh` CLI works.
- **zsh gotchas:**
  - `$var` is not word-split; use `${=spec}`.
  - An unquoted `echo ===X` triggers `=` expansion; quote it.
  - `grep --include=*.rs` globs fail; use `git grep … -- '*.rs'`.

## Patterns to reuse, not reinvent

- **Bug-fix recipe (used 3× this session):**
  1. Branch off `origin/main`. If you started on the wrong branch: `git stash -u; git checkout -b X origin/main; git stash pop`.
  2. Write the failing test, confirm red, fix, confirm green.
  3. Run test + clippy for each touched crate, then fmt.
  4. Hunt for doc drift: `git grep` the behaviour across `*.md`.
  5. Commit with `Closes #N`, push, open the PR.
  6. File any out-of-scope half as a new issue **before** writing the PR body, so the body can cite it.
- **Duplicated GUI+CLI logic → one pure helper in `primer-engine`** that takes the *result* of the I/O call, not the store. That keeps it mock-free to test, as with `adopt_or_mint_learner_id`.
- **Split recipe, substantive-line diff, Dependabot triage:** unchanged; see the handoff `docs/handoffs/2026-10-16T202752Z.md` § "Patterns to reuse".
- **Stacked PRs:** after the base squash-merges, retarget or rebase the child **before** merging it. Otherwise it lands on the dead base branch, which is exactly what happened to #344.
- Read `NEXT_SESSION.md` AFTER `git pull`, and check `git status` first.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status --short && git checkout main && git pull --ff-only && git log --oneline -6 && gh pr list --state open
# Open at close: #345, #346, #348, #349 (independent). Confirm #330/#319 closed once #345 merges.
S=/private/tmp/primer-scratch; mkdir -p "$S"   # or the session's scratchpad dir

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done

# === Next bug (#347): start here ===
gh issue view 347
git checkout -b fix/fallback-dropped-banner-347 origin/main
~/.cargo/bin/cargo test -p primer-engine -p primer-gui 2>&1 | grep 'test result'   # baseline

# === Standard workspace gate (one cargo pass at a time) ===
~/.cargo/bin/cargo test --workspace 2>&1 | tee $S/ws.log | tail -3
grep -E 'test result: FAILED|^error' $S/ws.log  # expect empty
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo fmt --all -- --check

# === Rustdoc gate (CI-enforced once #345 merges); feature arms mirror ci.yml ===
RUSTDOCFLAGS="-D warnings" ~/.cargo/bin/cargo doc --workspace --no-deps --document-private-items
for spec in "-p primer-speech --features voice-loop,silero,whisper,piper,cpal,supertonic" "-p primer-speech --features android-native" "-p primer-speech --features voice-loop,android-native" \
  "-p primer-inference --features qnn" "-p primer-inference --features llamacpp" "-p primer-gui --features speech" "-p primer-gui --features android-native" \
  "-p primer-cli --features speech" "-p primer-speech --features voice-loop,silero,cpal,macos-native" "-p primer-speech --features voice-loop,silero,cpal,macos-native-26"; do \
  RUSTDOCFLAGS="-D warnings" ~/.cargo/bin/cargo doc ${=spec} --no-deps --document-private-items >/dev/null 2>&1; echo "$spec exit=$?"; done
```

## Reporting back

- **#345:** re-lands #344, which had merged into its dead stacked base. rustdoc `-D warnings` is clean, and the `cargo doc` CI gate now passes on Linux as well as macOS. Closes #330 and #319.
- **#346 / #348 / #349:** TDD fixes for #339 (no silent learner-id mint on a DB error), #337 (the GUI now loads `~/.primer_env`) and #335 (match-main ignores a stale model). Each is a small diff with a red-then-green test.
- **New issue #347:** tell the GUI user when the cloud fallback was dropped. Recommended next.
