# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-10-16 20:30 UTC. **Three PRs open against `main`:**

| PR | Branch | What | Depends on |
|---|---|---|---|
| **#342** | `refactor/split-gui-voice-commands` | GUI `commands/voice.rs` split | — |
| **#343** | `chore/bump-rustls` | `rustls` bump for Dependabot #8/#9 | — |
| **#344** | `docs/fix-rustdoc-links` | rustdoc links + `cargo doc` CI gate; also carries this brief | **stacked on #342** |

`main` was at `0e74d8b` when this session branched. Both PRs from the prior brief (#333, #334) had merged between sessions.

This session ran the standard sweep first. The oversized-file sweep matched expectations, and the inline-test detector was **clean for the sixth session running**. Then it took the recommended split (`commands/voice.rs`), triaged two new Dependabot alerts, and closed issue #330 (plus #319) with a CI gate.

## What we shipped this session

### PR #342 (`refactor/split-gui-voice-commands`): `primer-gui/src/commands/voice.rs`, 559 lines
Commit **`58120eb`**. The file was N independent items (types + 9 Tauri commands + one helper), so this was a **mechanical** split, not a stage extraction like #333.

| File | Lines | Holds |
|---|---:|---|
| `voice/mod.rs` | 41 | doc + flat `pub use` glob façade |
| `voice/types.rs` | 108 | `kind` ids, `StartVoiceModeError`, `MissingAsset`, `missing_to_error` |
| `voice/start.rs` | 189 | `start_voice_mode` (+ stub) |
| `voice/stop.rs` | 118 | `stop_voice_mode`, `cancel_voice_response`, `stop_voice_mode_inner` (+ stubs) |
| `voice/asset_download.rs` | 57 | `download_voice_assets` (+ stub) |
| `voice/state_copy.rs` | 53 | `VoiceStateCopy` + `get_voice_state_copy` |
| `voice/capabilities.rs` | 54 | the four `*_available` flags |
| `voice/tests.rs` | 281 | untouched |

- **Tauri gotcha, now known.** `#[tauri::command]` on a `pub fn` emits `#[macro_export]` `__cmd__*` macros. The glob `pub use sub::*;` façade re-exports them, so `generate_handler![voice::…]` stays unchanged. `commands/session/` already relied on this.
- **Only two non-move edits:**
  - `super::session::` → `crate::commands::session::`;
  - `VoiceStateCopy::for_locale` → `pub(super)`, so `tests.rs` still reaches it.
- **Verified:**
  - pub surface identical (24 items); the substantive-line diff shows only those two edits plus façade/import lines;
  - tests: 217/217 default and **265/265 `--features speech`**, both equal to baseline;
  - clippy `-D warnings` clean for default, `speech`, `android-native` and `--no-default-features`;
  - workspace test 51 × ok;
  - **CI fully green.**

### PR #343 (`chore/bump-rustls`): Dependabot alerts #8 + #9
Commit **`932cf4b`**. `rustls` → **0.23.45** (GHSA-2mjx-qc3c-rqvc, medium: TLS 1.3 handshake messages accepted across encryption-level boundaries).
- Lockfiles: `src/Cargo.lock` (0.23.39) and `spikes/whisper_probe/Cargo.lock` (0.23.40). `rustls-webpki` goes to 0.103.15.
- The spike lockfile also re-synced resolver drift: it added a `serde` dependency entry for the `primer-speech` path dep and dropped a stale `[[patch.unused]] piper-rs` stanza.
- **Reachable, unlike #334.** rustls is the TLS stack for reqwest (cloud backend + downloads) and ureq (hf-hub).
- Verified: `cargo check --workspace --all-targets --locked` clean; spike `cargo metadata --locked` clean; primer-inference + primer-embedding tests 216/0. **CI fully green.**
- Once merged: **zero open Dependabot alerts** (#2 glib remains dismissed, tracked by #135).

### PR #344 (`docs/fix-rustdoc-links`, stacked on #342): closes #330 and #319
Commit **`598176d`**, plus a follow-up with the PR-number citations in CLAUDE.md and this brief. `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items` is now **clean on the default workspace build AND on every feature arm**:
- speech/supertonic, android-native ×2, qnn, llamacpp, gui speech, cli speech;
- macos-native and macos-native-26.

That was 47 default-arm failures plus about 45 in feature-gated code.
- **Façade modules use a scoped `#![allow(rustdoc::private_intra_doc_links)]`**, with a rationale comment, instead of escaping the links. Affected: engine `wiring`, gui `commands/session` and `config`, `primer-knowledge`, pedagogy `prompt_builder` and `prompt_pack`.
  - **This deliberately deviates from #330's "escape" suggestion.** The links work under `--document-private-items`, so escaping would discard the navigation the issue argued for.
  - Flagged in the PR body for the owner. If overruled, escaping is a mechanical follow-up.
- **Root cause found in `voice_loop/mod.rs`:** an outer `///` on a `mod` line combined with inner `//!` docs in the file makes rustdoc resolve all of it in the **parent's** scope. The fix was to demote the outer docs to `//`.
- **CI:** a `cargo doc` step in the `test` job, a multi-arm step in `feature-combos`, and a macos-native step in `feature-combos-macos`. `macos-native-26` is local-only.
- CLAUDE.md now has a convention entry with the 4 rules.
- Verified: all 10 arms clean; workspace clippy and fmt clean; `cargo test --workspace --doc` ok.

### Environment fix (no commit)
The first `--features speech` baseline failed with `'cstring' file not found` from the `espeak-rs-sys` C++ build.
- **Cause:** stale CMake caches in `target/` pinned `-isysroot …/MacOSX26.5.sdk`, which the Xcode upgrade removed; only `MacOSX27.0.sdk` remains.
- **Fix:** `cargo clean -p espeak-rs-sys -p whisper-cpp-plus-sys -p llama-cpp-sys-2` (and `--release` for the first two). This is a local artifact issue, not a repo bug. The recipe is under *Open decisions / risks*.

## What's next (concrete acceptance criteria)

### 0. Owner review/merge: #342, #343, #344
- **#343** is independent; merge any time.
- **Merge #342 before #344.** After #342 squash-merges, GitHub retargets #344 to `main`. If #344 then shows #342's commit or conflicts, rebase it:
  ```bash
  git fetch origin && git checkout docs/fix-rustdoc-links && git rebase --onto origin/main 58120eb && git push --force-with-lease
  ```
- **Acceptance:** all CI green, owner merges.
- **After #344 merges,** confirm the new `cargo doc` CI steps pass on `main`. They were verified locally on macOS only, so the Linux `feature-combos` arm runs for the first time in CI.

### 1. Sweep + inline-test detector: re-run EVERY session (clean six sessions running)
Commands are in the resume block. Watch-list: `primer-classifier/src/llm.rs` (441) and `primer-extractor/src/llm.rs` (454).

### 2. Production-code splits (the open, owner-approved lane; no re-ask needed)
Remaining oversized production files. **All are feature-gated, so dual-verify.**
- **`primer-inference/src/qnn/genie/real.rs` (566, `qnn`)**: recommended next.
  - Baseline `cargo test -p primer-inference` plus `cargo clippy -p primer-inference --features qnn --all-targets -- -D warnings`.
  - The real arm is Android-only at runtime, so the host can only compile it.
  - Also run the new rustdoc arm: `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items -p primer-inference --features qnn`.
- **`primer-speech/src/macos/{tts.rs 668, stt.rs 504}`** (`macos-native`; this host is macOS).
- `primer-speech/src/voice_loop/state_machine/inner.rs` (506): one big function and only 6 lines over. Low value for the risk; consider leaving it.
- Hardest: `primer-cli/src/main.rs` (1357, heavily cfg-gated).
- **Off the list:** `consts.rs`, `prompt_builder.rs`, `dialogue_manager/turn.rs`, `state_machine.rs`, `schema.rs`, `config/types.rs`, `wiring.rs`, `commands/voice.rs` (#342).
- **New gate for every split:** the rustdoc arms (CI now enforces them). If a split gives a module both an outer `///` and an inner `//!`, links break; see the CLAUDE.md rule.

### 3. Open bugs filed by the #333 review: a good alternative to splits
The highest-value open bugs, all host-completable:
- **#339:** learner-id adoption: a DB error mints a fresh UUID and orphans sessions (GUI + CLI).
- **#337:** the GUI never loads `~/.primer_env`, so the opt-in cloud fallback is silently dropped.
- **#335:** "match main" clears the subsystem kind but not the model override.
- **#338:** the sidebar shows the local primary while the cloud fallback serves turns.
- **#336:** `eprintln!` instead of `tracing::warn!` for embedder fallback notices.
- **#341:** CLI-flag wording in engine errors reaches GUI users.
- **#340:** stage-test gaps.

Older ones: #307–#316 (retrieval/storage/streaming), #325/#326 (voice), #331/#332 (storage).

Acceptance per bug:
- a failing test first (TDD);
- the fix;
- the `Closes #N` PR.

### Carried / owner-or-hardware-gated (none host-completable autonomously)
- **#260**: Android-voice on-device acceptance.
- **#192**: manual macOS-native STT + injected TTS smoke.
- **#170 Stages B, E and F**: the Supertonic OpenRAIL-M disclosure must ship before any default flip.
- **#166 item #1**: owner-run WhisperStream reuse smoke.
- **#135**: glib (blocked on Tauri 3).
- **Branch protection**: make `cargo test (default features)` a required check.
- QNN token stability, NPU tuning, latency-routing calibration.

## Open decisions / risks

- **#344's façade-`allow` choice vs. #330's "escape" suggestion.** This is the owner's call, and it is flagged in the PR body.
- **#344 is stacked on #342, and `main` squash-merges.** A rebase may be needed; the command is in §0.
- **New `cargo doc` CI steps are unproven on Linux CI.** They pass locally on macOS. Risks:
  - the `feature-combos` arm documents `primer-speech` with `whisper`/`piper`/`supertonic` and `primer-gui --features speech` on Ubuntu. Those deps are already built there by clippy steps, but a Linux-only cfg arm could surface a link that macOS never compiles;
  - a doc build compiles deps in a different profile, which could add build time.

  Watch the first #344 CI run.
- **Stale CMake cache after Xcode/SDK upgrades.** Symptom: C++ `'<header>' file not found` from a `*-sys` build script. Recipe:
  1. Find the stale caches: `find src/target -name CMakeCache.txt -maxdepth 6 | xargs grep -l 'MacOSX<old>'`.
  2. `cargo clean -p <those sys crates>`.
- **`github` MCP server failed to connect** (bad Authorization header). The `gh` CLI works.
- **Build times (warm):** `cargo test -p primer-gui` ~25 s, `--features speech` ~30 s after the first build, workspace test ~7 min, the 10-arm rustdoc matrix ~10+ min (llamacpp/whisper builds). Run ONE cargo pass at a time, and never edit sources while a baseline compiles.
- **zsh does not word-split `$var`.** In loops over cargo arg strings, use `${=spec}`. This session lost one matrix run to it.

## Patterns to reuse, not reinvent

### Split recipe
1. **Baseline** `cargo test -p <crate>` (plus gating `--features`) BEFORE touching anything; record the counts.
2. **Check the shape:**
   - N independent items → mechanical (#342);
   - one big function → stage extraction + stage tests (#333).
3. **Draft in the scratchpad** while the baseline compiles: `git show main:<path> > $S/orig.rs`, then `sed -n 'A,Bp'` byte-exact ranges into `$S/new/*.rs`. Run the line and pub diffs on the drafts. Move them in after the baseline finishes.
4. **Visibility across the new boundary**, six cases seen so far:
   1. already-`pub mod`: no work;
   2. a parent that globs you: `pub use sub::*;`;
   3. private items a test child reaches: keep them in `mod.rs`, or give them `pub(super)`;
   4. a `pub(super)` method that parent-level tests call: `pub(in crate::<path>)`;
   5. a sibling-moved helper reached via `use super::*`: `pub(super)` + `#[cfg(test)] use sub::name;`;
   6. a mixed-visibility explicit façade: one re-export per visibility (glob façades sidestep this).

   Tauri `pub` commands re-export fine through globs.
5. **Fix paths:** `super::x` becomes `crate::…` when code moves one level deeper.
6. **Verify:**
   - pub-surface diff;
   - substantive-line diff (audit every line);
   - test counts equal the baseline, per feature arm;
   - clippy for every cfg arm the moved code has;
   - **rustdoc `-D warnings` arms**;
   - workspace test;
   - fmt `--check`.

### Substantive-line diff
```bash
strip() { grep -vE '^\s*//' | grep -vE '^\s*$' | grep -vE '^\s*(pub |pub\(crate\) |pub\(super\) )?(use |mod ) ?' | sed -E 's/^\s+//'; }
strip < $S/orig.rs | sort > $S/old.txt
cat <new files> | strip | sort > $S/new.txt
diff $S/old.txt $S/new.txt   # audit EVERY line
```

### Dependabot triage recipe
1. `gh api 'repos/hherb/primer/dependabot/alerts?state=open' --jq '.[] | {n:.number, sev:.security_advisory.severity, pkg:.dependency.package.name, fix:.security_vulnerability.first_patched_version.identifier, manifest:.dependency.manifest_path}'`. **Note `manifest`:** `spikes/whisper_probe/Cargo.lock` gets alerts too.
2. Check reachability: `cargo tree -i <pkg> --workspace --all-features --target all`. Empty output means lockfile-only.
3. On a branch off `main`: `cargo update -p <pkg> --precise <fix>`, in each affected lockfile's dir.
4. Verify: `cargo check --workspace --all-targets --locked`, plus `cargo metadata --locked` for the spike.

### Other habits
- **Doc-drift hunt:** grep the moved file name AND its key symbols across README/ROADMAP/CLAUDE/docs and `*.rs` doc comments.
- **Cite the PR number in CLAUDE.md** once the PR exists. In a stack, put that commit on the TOP branch, so the base PR's squash stays rebase-clean.
- Read `NEXT_SESSION.md` AFTER `git pull`. Check `git status` first: this session found a stray working-tree `Cargo.lock` that *reverted* the #334 security bump, and restored it with `git checkout src/Cargo.lock`.
- A trailing `grep` that matches nothing returns exit 1; read the `test result:` lines, not the exit code. To wait for cargo, use an `until ! pgrep -f '<pattern>'` loop. Note that clippy runs as `cargo-clippy`, so match `primer-gui` or `cargo-clippy`, not `cargo clippy`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status --short && git checkout main && git pull --ff-only && git log --oneline -5 && gh pr list --state open
# Re-read NEXT_SESSION.md (it may have changed in the pull). Open at close: #342, #343, #344 (stacked on #342).
S=/private/tmp/primer-scratch; mkdir -p "$S"   # or the session's scratchpad dir

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done

# === Recommended next split: primer-inference/src/qnn/genie/real.rs (566, qnn-gated). Baselines FIRST: ===
~/.cargo/bin/cargo test -p primer-inference 2>&1 | grep 'test result: ok'
~/.cargo/bin/cargo clippy -p primer-inference --features qnn --all-targets -- -D warnings

# === Standard workspace gate (one cargo pass at a time) ===
~/.cargo/bin/cargo test --workspace 2>&1 | tee $S/ws.log | tail -3
grep -cE 'test result: ok' $S/ws.log            # expect 51
grep -E 'test result: FAILED|^error' $S/ws.log  # expect empty
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo fmt --all -- --check

# === Rustdoc gate (CI-enforced once #344 merges); feature arms mirror ci.yml ===
RUSTDOCFLAGS="-D warnings" ~/.cargo/bin/cargo doc --workspace --no-deps --document-private-items
for spec in "-p primer-speech --features voice-loop,silero,whisper,piper,cpal,supertonic" "-p primer-speech --features voice-loop,android-native" \
  "-p primer-inference --features qnn" "-p primer-inference --features llamacpp" "-p primer-gui --features speech" "-p primer-gui --features android-native" \
  "-p primer-cli --features speech" "-p primer-speech --features voice-loop,silero,cpal,macos-native" "-p primer-speech --features voice-loop,silero,cpal,macos-native-26"; do \
  RUSTDOCFLAGS="-D warnings" ~/.cargo/bin/cargo doc ${=spec} --no-deps --document-private-items >/dev/null 2>&1; echo "$spec exit=$?"; done

# === Pub-surface diff ===
git show main:src/crates/<path>.rs | grep -oE 'pub (struct|enum|fn|const|async fn|trait|type|mod) [A-Za-z_0-9]+' | sort -u > $S/old-pub.txt
cat <new files> | grep -oE 'pub (struct|enum|fn|const|async fn|trait|type|mod) [A-Za-z_0-9]+' | sort -u > $S/new-pub.txt
diff $S/old-pub.txt $S/new-pub.txt

# === Carried: owner-run the #166 reuse smoke (needs a model + two 16 kHz mono WAVs) ===
PRIMER_WHISPER_MODEL=/path/to/ggml-small.en.bin \
PRIMER_WHISPER_AUDIO_A=/path/to/utterance_a.wav \
PRIMER_WHISPER_AUDIO_B=/path/to/utterance_b.wav \
  ~/.cargo/bin/cargo test -p primer-speech --features whisper \
  --test whisper_stream_reuse -- --ignored --nocapture
```

## Reporting back

- **#342 (`commands/voice.rs` split):** 559 lines became 6 submodules plus a 41-line façade. Mechanical split; tests at 217 default / 265 speech, both equal to baseline; CI green.
- **#343 (rustls → 0.23.45):** clears Dependabot #8/#9. This one is reachable (it is our TLS stack); CI green. After merge, zero open alerts.
- **#344 (rustdoc):** closes #330 + #319. rustdoc `-D warnings` is clean on all 10 arms, and a `cargo doc` CI gate is added across three jobs. Façade links are kept through a scoped `allow` (owner call). Stacked on #342.
- **Next:** the `qnn/genie/real.rs` split, or a TDD fix for #339 / #337 / #335.
