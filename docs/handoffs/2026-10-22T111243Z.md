# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-10-22 11:12 UTC. **One PR is open against `main`:**

| PR | Branch | What | Closes |
|---|---|---|---|
| **#356** | `fix/gui-retrieval-notice-353` | GUI banner when the configured embedder falls back to BM25-only. Also carries this brief. | #353 |

`main` is at `5df0fcb` (#354). Since the last brief, **#354 merged**, and **#336 is confirmed CLOSED**.

## What we shipped this session

### #356 (`6e762b0`): closes #353
**GUI (`primer-gui`):**
- New pure `wiring/subsystems.rs::retrieval_notice(kind, built) -> Option<String>`.
  - Returns `None` when the embedder built, or when `kind == "none"` (BM25-only by choice).
  - Otherwise returns an English, operator-facing notice that names the embedder.
- `wiring/mod.rs` computes the notice from `config.embedder.kind` and `subsystems.embedder.is_some()`. **No engine signature changed.**
- `ActiveSession.retrieval_notice` and `SessionInfo.retrieval_notice` carry it.
  - All four `SessionInfo` construction sites forward it: `session/lifecycle.rs::info_from`, `voice/start.rs`, `voice_android.rs`, and the `voice/tests.rs` fixture.
- **UI:** new `#retrieval-notice-banner` in `index.html`, filled via `textContent` in `app.js::renderSessionInfo`.
  - It is hidden on reset in both places where the backend banner is.
  - Voice mode re-renders through `window.primerRenderSessionInfo`, so it stays current.

**TDD:**
- Stage tests: `retrieval_notice_absent_when_intentional_or_built` and `retrieval_notice_reports_failed_embedder`.
- End-to-end: `working_or_disabled_embedder_has_no_retrieval_notice` (`none` and `stub`).
- Red was compile errors; green is 225 passed in `primer-gui`.

**Docs:** CLAUDE.md, `docs/devel/04-knowledge-and-retrieval.md`, README (the hybrid-retrieval bullet), and the `primer-engine` `embedders.rs` doc comment. ROADMAP needed nothing.

**Gate results:**
- `cargo test -p primer-gui`: 225/0.
- clippy `-D warnings`: clean for the workspace, for `-p primer-gui --features speech`, and for `-p primer-gui --features android-native`.
- fmt: clean.
- rustdoc `-D warnings`: clean for `primer-gui` (default, speech, android-native) and for `primer-engine`.
- **Not run:** the full `cargo test --workspace`. Only `primer-gui` code changed; the engine change was a doc comment.

### Per-session sweep
- The oversized list is unchanged.
- The **inline-test detector is clean for the tenth session running**.
- Watch-list: `primer-classifier/src/llm.rs` (441), `primer-extractor/src/llm.rs` (454), `primer-engine/src/wiring/backend.rs` (443).

## What's next (concrete acceptance criteria)

### 0. Owner review/merge: #356
- **Acceptance:** CI is green and the owner merges.
- Then confirm #353 auto-closed: `gh issue view 353 --json state`.

### 1. Sweep + inline-test detector: re-run EVERY session
Commands are in the resume block.

### 2. Remaining review-filed bugs (host-completable; TDD each: failing test, fix, `Closes #N` PR)
- **#355:** desktop-GUI tracing goes to stdout, which a Dock launch discards.
  - It pairs naturally with #353: the banner now says *that* the embedder failed, but the *why* is still lost on a Dock launch.
  - **Acceptance:**
    - the desktop GUI writes tracing to a file sink under the app's data dir (or stderr plus a file);
    - a test pins the writer choice;
    - Android logcat behaviour is unchanged.
- **#341:** CLI-flag wording in engine errors reaches GUI users. This also covers `BackendNotice.reason`.
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

- **The #353 failed-embedder case is pinned only via the pure helper.**
  - The end-to-end GUI test covers only `none` and `stub`. A real failure needs network control (fastembed) or a GUI `ollama-embedding` feature arm plus a new CI step.
  - The wiring line is a one-liner (`subsystems.embedder.is_some()`), so the risk is low.
- **Banner not seen live.** The new banner hasn't been checked in `cargo tauri dev`. It uses the same `error-banner` class as `#backend-notice-banner`.
- **Notice text is English-only** and operator-facing, like `BackendNotice`.
- **CLI tracing goes to stderr** (since #354's review fix). Desktop-GUI tracing still goes to stdout, which a Dock launch discards: #355.
- **The fastembed failure path is still untested** (it can't be forced without network control).
- **Carried from #350:** the notice `reason` is the raw construction error (wording cleanup is #341); a real Dock launch of the bundled `.app` is unexercised.
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
# Open at close: #356. After merge, confirm #353 closed: gh issue view 353 --json state
S=/private/tmp/primer-scratch; mkdir -p "$S"   # or the session's scratchpad dir

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done

# === Next bug (#355): start here ===
gh issue view 355
git checkout -b fix/gui-tracing-sink-355 origin/main
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

- **#356:** the desktop app now shows a persistent banner when the configured embedder fails to start and knowledge search drops to keyword-only BM25.
  - A configured `none` stays silent.
  - It closes #353 with red-then-green tests; the `primer-gui` gate is green (225 tests) across the default, speech and android-native arms.
- #354 merged; #336 is closed.
