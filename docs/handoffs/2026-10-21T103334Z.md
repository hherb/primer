# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-10-21 10:33 UTC. **One PR is open against `main`:**

| PR | Branch | What | Closes |
|---|---|---|---|
| **#350** | `fix/fallback-outcome-347-338` | A degraded fallback/router leg is reported to the user, not just logged. Also carries this brief. | #347, #338 |

`main` is at `e42628f` (#349). Since the last brief, **all four PRs merged**: #345, #346, #348 and #349. **#330 and #319 are confirmed CLOSED.**

## What we shipped this session

### #350 (`68098fd`): closes #347 and #338
**Engine:** `primer_engine::build_main_backend` now returns `Result<(Arc<dyn InferenceBackend>, Option<BackendNotice>)>`. This applies to the fallback path **and** the router path.
- `BackendNotice::FallbackDropped { fallback, reason }`: plan `PrimaryAlone`, so there is no safety net.
- `BackendNotice::PrimaryUnavailable { primary, fallback, fallback_model, reason }`: plan `SecondaryAlone`, so the fallback serves every turn.
- `Display` is the user-facing line.
- New private helpers in `wiring/backend.rs` (443 lines): `build_secondary_leg` dedups the leg build shared by the fallback and router paths; `degraded_notice` maps a plan to a notice.

**CLI:** prints `Warning: <notice>` on stderr.

**GUI:**
- `ActiveSession.backend_notice` and `SessionInfo.backend_notice` (`Option<String>`) feed a new persistent `#backend-notice-banner` in `index.html`.
- `renderSessionInfo` sets it with `textContent`; `resetSessionHeader` hides it.
- The pure `wiring/backend_params.rs::served_identity` swaps the sidebar's kind/model to the fallback on `PrimaryUnavailable` (#338). `main_model` stays the subsystem identifier.

**Drive-by:** moved `build_backend_params`'s doc comment back onto it (#348 had left it stacked above `env_key`).

**Docs:** CLAUDE.md (the FallbackBackend bullet), README (§ Inference routing), ROADMAP (line 64).

**TDD:** 5 new engine tests and 4 new GUI tests.

**Gate results:**
- workspace tests: 1336 passed, 0 failed;
- clippy `-D warnings`: clean for the workspace, `primer-gui` with `speech` and with `android-native`, and `primer-cli` with `speech`;
- fmt and default-arm rustdoc `-D warnings`: clean.

### Per-session sweep
- The oversized list is unchanged.
- The **inline-test detector is clean for the eighth session running**.
- Watch-list: `primer-classifier/src/llm.rs` (441), `primer-extractor/src/llm.rs` (454), `primer-engine/src/wiring/backend.rs` (443, grew this session).

## What's next (concrete acceptance criteria)

### 0. Owner review/merge: #350
- **Acceptance:** CI green and the owner merges.
- Then confirm #347 and #338 auto-closed.
- **Optional manual check:** `cargo tauri dev` with primary `ollama` (not running) and fallback `ollama` without a model → the banner shows "Fallback backend 'ollama' is disabled…".

### 1. Sweep + inline-test detector: re-run EVERY session
Commands are in the resume block.

### 2. Remaining review-filed bugs (host-completable; TDD each: failing test, fix, `Closes #N` PR)
- **#336:** use `tracing::warn!` instead of `eprintln!` for embedder fallback notices. Small; a good next pick.
- **#341:** CLI-flag wording in engine errors reaches GUI users. This now also covers `BackendNotice.reason`, which is the raw construction error (e.g. "--fallback-model is required…").
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

- **#350 shows the raw construction error as the notice `reason`.** The GUI already did the same for fatal construction errors ("constructing inference backend: {e}"), so the posture is unchanged. Wording cleanup belongs to #341.
- **The #350 banner is English-only.** So is the rest of the GUI chrome. `BackendNotice` is typed with locale-neutral fields, so a future i18n render is mechanical.
- **The CLI's "Using X backend" line still names the configured primary** when the fallback serves alone. The `Warning:` line printed right above it explains this, so I deliberately left the line alone.
- **Not exercised:** the banner in a live `cargo tauri dev`; a real Dock launch of the bundled `.app` (carried from #348).
- **Stale CMake cache after Xcode/SDK upgrades.** Symptom: `'<header>' file not found` from a `*-sys` build. Recipe:
  1. Find the stale caches: `find src/target -name CMakeCache.txt -maxdepth 6 | xargs grep -l 'MacOSX<old>'`.
  2. `cargo clean -p <those sys crates>`.
- **The `github` MCP server fails to connect** (bad Authorization header). The `gh` CLI works.
- **zsh gotchas:**
  - `$var` is not word-split; use `${=spec}`.
  - An unquoted `echo ===X` triggers `=` expansion; quote it.
  - `grep --include=*.rs` globs fail; use `git grep … -- '*.rs'`.

## Patterns to reuse, not reinvent

- **Bug-fix recipe:**
  1. Branch off `origin/main`.
  2. Write the failing test, confirm red, fix, confirm green.
  3. Run test + clippy for each touched crate (plus the feature arms whose files you touched), then fmt.
  4. Hunt for doc drift: `git grep` the behaviour across `*.md`.
  5. Commit with `Closes #N`, push, open the PR.
  6. File any out-of-scope half as a new issue before writing the PR body.
- **Two issues needing the same plumbing → one PR.** #347 (banner) and #338 (sidebar identity) both needed `build_main_backend` to expose its plan outcome, so one typed return value fixed both.
- **Changing a widely-called signature:** mechanically rewrite test call sites with `perl -pi` (`let b =` → `let (b, _) =`), so the red phase is pure compile errors pointing at the missing type.
- **Duplicated GUI+CLI logic → one pure helper in `primer-engine`** that takes the *result* of the I/O call, not the store (`adopt_or_mint_learner_id`).
- **Split recipe, substantive-line diff, Dependabot triage:** unchanged; see `docs/handoffs/2026-10-16T202752Z.md` § "Patterns to reuse".
- **Stacked PRs:** after the base squash-merges, retarget or rebase the child **before** merging it (the #344 loss).
- Read `NEXT_SESSION.md` AFTER `git pull`, and check `git status` first.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status --short && git checkout main && git pull --ff-only && git log --oneline -6 && gh pr list --state open
# Open at close: #350. After merge, confirm #347/#338 closed: gh issue view 347 --json state; gh issue view 338 --json state
S=/private/tmp/primer-scratch; mkdir -p "$S"   # or the session's scratchpad dir

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done

# === Next bug (#336): start here ===
gh issue view 336
git checkout -b fix/embedder-fallback-tracing-336 origin/main
~/.cargo/bin/cargo test -p primer-engine -p primer-cli -p primer-gui 2>&1 | grep 'test result'   # baseline

# === Standard workspace gate (one cargo pass at a time) ===
~/.cargo/bin/cargo test --workspace 2>&1 | tee $S/ws.log | tail -3
grep -E 'test result: FAILED|^error' $S/ws.log  # expect empty
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo fmt --all -- --check

# === Rustdoc gate (CI-enforced); feature arms mirror ci.yml ===
RUSTDOCFLAGS="-D warnings" ~/.cargo/bin/cargo doc --workspace --no-deps --document-private-items
for spec in "-p primer-speech --features voice-loop,silero,whisper,piper,cpal,supertonic" "-p primer-speech --features android-native" "-p primer-speech --features voice-loop,android-native" \
  "-p primer-inference --features qnn" "-p primer-inference --features llamacpp" "-p primer-gui --features speech" "-p primer-gui --features android-native" \
  "-p primer-cli --features speech" "-p primer-speech --features voice-loop,silero,cpal,macos-native" "-p primer-speech --features voice-loop,silero,cpal,macos-native-26"; do \
  RUSTDOCFLAGS="-D warnings" ~/.cargo/bin/cargo doc ${=spec} --no-deps --document-private-items >/dev/null 2>&1; echo "$spec exit=$?"; done
```

## Reporting back

- **#350:** a configured fallback or router leg that fails to build at startup is now reported, not just logged. The CLI prints a `Warning:` line. The GUI shows a persistent banner, and its sidebar names the backend that is actually answering. Closes #347 and #338, with red-then-green tests; the workspace gate is green (1336 tests).
- Last session's four PRs (#345, #346, #348, #349) all merged; #330 and #319 are closed.
