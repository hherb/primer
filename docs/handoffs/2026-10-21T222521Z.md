# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-10-21 22:25 UTC. **One PR is open against `main`:**

| PR | Branch | What | Closes |
|---|---|---|---|
| **#354** | `fix/embedder-fallback-tracing-336` | Embedder BM25 fallback and ignored-model notices go through `tracing`, not `eprintln!`. Also carries this brief. | #336 |

`main` is at `1583ab4` (#350). Since the last brief, **#350 merged**, and **#347 and #338 are confirmed CLOSED**.

## What we shipped this session

### #354 (`225c751`): closes #336
**Engine (`primer-engine/src/wiring/embedders.rs`):**
- Embedder init failure ("… falling back to BM25-only retrieval") now uses `tracing::warn!`, for fastembed, ollama and openai-compat.
- The ignored non-bge-m3 model note also uses `tracing::warn!`.
- The "Loading fastembed…" and "Embedder: … at …" lines now use `tracing::info!`.
- The CLI's default filter (`info,…`) keeps all of these visible. The GUI's subscriber and log sink now receive them.
- The model note drops its CLI-only `--embedder-model` wording, because the GUI calls the same builder.

**TDD:** new `wiring/embedder_fallback_tests.rs`.
- It points real ollama and openai-compat embedders at `http://127.0.0.1:1` (a closed port, so there is no network) and captures the subscriber output.
- Red on `eprintln!`, green on `tracing`.
- The module is gated on `cfg(all(test, any(ollama-embedding, openai-compat-embedding)))`, so its helpers are never dead code on a default build.
- New dev-dep: `tracing-subscriber` (already a workspace dep).

**CI:** new `feature-combos` step: `cargo test -p primer-engine --features ollama-embedding,openai-compat-embedding embedder_fallback`. Before this, no CI job built those features.

**Docs:** none needed. CLAUDE.md and `docs/devel/04-knowledge-and-retrieval.md` already said `tracing::warn!`; the code now matches them.

**Gate results:**
- workspace tests: 1338 passed, 0 failed;
- clippy `-D warnings`: clean for the workspace, and for `primer-engine` with the two embedding features;
- fmt and `primer-engine` rustdoc `-D warnings`: clean.

**Filed:** **#353**, the GUI indicator for the effective retrieval mode ("BM25-only (embedder unavailable)"). This was #336's optional half; it mirrors the #347 `backend_notice` pattern.

### Per-session sweep
- The oversized list is unchanged.
- The **inline-test detector is clean for the ninth session running**.
- Watch-list: `primer-classifier/src/llm.rs` (441), `primer-extractor/src/llm.rs` (454), `primer-engine/src/wiring/backend.rs` (443).

## What's next (concrete acceptance criteria)

### 0. Owner review/merge: #354
- **Acceptance:** CI is green, including the new feature-combos step, and the owner merges.
- Then confirm #336 auto-closed.

### 1. Sweep + inline-test detector: re-run EVERY session
Commands are in the resume block.

### 2. Remaining review-filed bugs (host-completable; TDD each: failing test, fix, `Closes #N` PR)
- **#353:** GUI retrieval-mode notice. Good next pick, since it reuses the #347 banner plumbing.
  - **Acceptance:**
    - `build_embedder` reports when a configured real embedder failed;
    - `ActiveSession`/`SessionInfo` carry it;
    - the GUI renders it;
    - a configured `none` shows no notice;
    - a pure stage test pins both cases.
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

- **CLI output stream moved.** The embedder lines now print through the CLI's `tracing_subscriber::fmt()`, which writes to **stdout**, not stderr. This is the same place the CLI's other `tracing::info!`/`warn!` lines already go. A user who sets `RUST_LOG=warn` will no longer see the info-level "Loading fastembed…" line; the warnings stay visible.
- **The fastembed failure path itself is still untested.** It needs a failing BGE-M3 init, which can't be forced without network control. Only the ollama and openai-compat arms are test-pinned; the fastembed arm is the same macro swap.
- **Carried from #350:**
  - The notice `reason` is the raw construction error (wording cleanup is #341).
  - The banner is English-only.
  - Not exercised: the banner in a live `cargo tauri dev`, and a real Dock launch of the bundled `.app`.
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
- **Two issues needing the same plumbing → one PR.** One typed return value can fix both (#347 + #338).
- **Changing a widely-called signature:** mechanically rewrite test call sites with `perl -pi`, so the red phase is pure compile errors.
- **Duplicated GUI+CLI logic → one pure helper in `primer-engine`** that takes the *result* of the I/O call (`adopt_or_mint_learner_id`).
- **Split recipe, substantive-line diff, Dependabot triage:** unchanged; see `docs/handoffs/2026-10-16T202752Z.md` § "Patterns to reuse".
- **Stacked PRs:** after the base squash-merges, retarget or rebase the child **before** merging it (the #344 loss).
- Read `NEXT_SESSION.md` AFTER `git pull`, and check `git status` first.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status --short && git checkout main && git pull --ff-only && git log --oneline -6 && gh pr list --state open
# Open at close: #354. After merge, confirm #336 closed: gh issue view 336 --json state
S=/private/tmp/primer-scratch; mkdir -p "$S"   # or the session's scratchpad dir

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done

# === Next bug (#353): start here ===
gh issue view 353
git checkout -b fix/gui-retrieval-notice-353 origin/main
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

- **#354:** embedder BM25-fallback notices now go through `tracing`, so they reach the GUI's log sink and not just an invisible stderr. Closes #336 with red-then-green tests and a new CI step; the workspace gate is green (1338 tests).
- Filed **#353** for an in-app "BM25-only" indicator.
- #350 merged; #347 and #338 are closed.
