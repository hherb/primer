# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-10-16 11:02 UTC. Branch `refactor/split-gui-wiring`. **Two PRs open against `main`, both branched independently off `main`: #333 (GUI `wiring.rs` stage split) and #334 (`quinn-proto` lockfile bump for Dependabot alert #6).** `main` was at `b47bf89` when this session branched; every PR from the prior brief (#322, #323, #324, #327, #328, #329) had merged between sessions. Issue **#330** (43 broken rustdoc intra-doc links) is still open.

This session ran the standard sweep first. The oversized-file sweep matched expectations, and the inline-test detector was **clean for the fifth session running**. `schema.rs` (#328) and `config/types.rs` (#329) had already been split between sessions, so this session took the remaining no-feature-gate production file, `primer-gui/src/wiring.rs`. The prior brief had flagged it as *not mechanical*, so it got stage-level verification plus new tests (details below).

## What we shipped this session

### PR #333 (branch `refactor/split-gui-wiring`): `primer-gui/src/wiring.rs`, 591 lines
Commits **`7b58445`** (the split + stage tests + CLAUDE.md) and a follow-up commit citing the PR number in CLAUDE.md (bundled with this brief).

The file was **one ~395-line `build_with_strategy` function** with 11 `// ─── stage ───` banners, plus ~90 lines of helpers. The stages became functions in sibling submodules; `mod.rs` is now a thin orchestrator that calls them in the original order:

| File | Lines | Holds |
|---|---:|---|
| `wiring/mod.rs` | 212 | 2 `pub` entry points, private `LocaleStrategy`, orchestrating `build_with_strategy` |
| `wiring/backend_params.rs` | 173 | `build_backend_params(&GuiConfig) -> BackendParams` (both API-key env reads) + `resolve_main_model` / `cloud_backend_in_use` / `subsystem_kind` |
| `wiring/stores.rs` | 185 | `open_session_store_and_learner(..) -> (SqliteSessionStore, Locale, LearnerModel)` + `open_knowledge_base` |
| `wiring/subsystems.rs` | 145 | `build_subsystems(..) -> DialogueManagerSubsystems` + private `build_embedder` |
| `wiring/stage_tests.rs` | 236 | **new:** 10 per-stage unit tests |
| `wiring/tests.rs` | 423 | **untouched** (end-to-end) |

- **Visibility:** the stage functions are `pub(super)`. `LocaleStrategy` stays plain-private in `mod.rs`; the child `stores.rs` reaches it via `super::LocaleStrategy` for free, since children can see their ancestors' private items. `cloud_backend_in_use` is no longer called from `mod.rs`, so it gets a `#[cfg(test)] use backend_params::cloud_backend_in_use;` re-import for `tests.rs` (case 5 of the recipe).
- **Verification beyond the pub-surface diff:** the pub-surface diff is nearly meaningless for this kind of split, since everything that moved was inside one private fn. So I ran the **order-insensitive substantive-line diff** and audited every difference. All of them were:
  - parameter renames that keep the same value;
  - new signatures and `Ok(..)` wrapping;
  - the embedder key now reading `params.openai_compat_api_key.clone()`, the same value as before.

  No literal, error message, `tracing` call or match arm changed. Side-effect order is unchanged; the only reorder is `VocabSettings` vs `PedagogyConfig`, both pure.
- **Stage tests** (`stage_tests.rs`):
  - a stale subsystem `kind` is ignored while `match_main` is set; explicit overrides pass through;
  - inline keys are used whatever the backend kind; the env cloud key is not read when cloud is unreachable (deterministic, no env mutation);
  - reasoning-marker parsing; fallback/router/TTFT passthrough;
  - fresh-learner minting under the cfg locale; the resume strategy re-tags the store to the persisted locale;
  - the in-memory KB is auto-seeded;
  - subsystems built on a stub main backend pick up all three GUI timeouts and the vocab cap (real `Arc::clone` inheritance is untested — #340); the vocab cap falls back to `DEFAULT_VOCAB_MAX_PER_PROMPT`.

  Every test sets `cfg.embedder.kind = "none"`, because the default build's embedder is fastembed (a ~570 MB download).

### PR #334 (branch `chore/bump-quinn-proto`): Dependabot alert #6
Commit **`c14cabf`**, a two-line `Cargo.lock` bump of `quinn-proto` 0.11.14 → 0.11.15, which clears GHSA-4w2j-m93h-cj5j (high: memory exhaustion from unbounded out-of-order stream reassembly).
- **Not exploitable today.** `quinn-proto` is a lockfile-only entry, reachable only via `reqwest`'s optional `http3` feature (which also needs `--cfg reqwest_unstable`). Neither locked `reqwest` enables it: 0.12.28 (ours: json/stream/rustls-tls) and 0.13.3 (via tauri: json/stream). `cargo tree -i quinn-proto --workspace --all-features --target all` prints nothing, and no quinn artifact exists in `target/`.
- `cargo check --workspace --all-targets --locked` clean; nothing recompiled.
- **Dependabot state after this lands: zero open alerts.** History: #1/#3 openssl, #4 tar and #7 serde_with (merged dependabot PR #323) are all fixed. **#2 glib was dismissed earlier as `tolerable_risk`**; it is tracked by issue #135 (glib 0.18 → 0.20+ is blocked on Tauri 3).

### Verification (#333)
- `cargo test -p primer-gui`: baseline **207** → **217 passed / 0 failed** (+10 new).
- `cargo test --workspace`: **51 × `test result: ok`**, 0 FAILED/error. This ran before the stage tests were added; only `primer-gui` changed afterwards, and its suite and clippy were re-run.
- `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo clippy -p primer-gui --all-targets -- -D warnings` clean after the tests landed; `cargo fmt --all -- --check` clean; `cargo check -p primer-gui --no-default-features --all-targets` clean.
- CI at close: 2 checks passing (Cloudflare Pages, gradle wrapper), 8 pending, **none failing**.

### Docs
- **CLAUDE.md:** four stale `wiring.rs::…` paths repointed (`resolve_main_model` and `build_backend_params` in `backend_params.rs`; `build_with_strategy` in `wiring/mod.rs`). A new paragraph in the `primer-gui` section describes the layout and the stage-test rationale (cites PR #333).
- **README.md / ROADMAP.md deliberately unchanged:** they describe capabilities, not file layout. Grep-verified: no reference to `wiring.rs`, `build_with_strategy` or `resolve_main_model`.

## What's next (concrete acceptance criteria)

### 0. PRs #333 and #334: owner review/merge
#333 is a pure refactor plus tests, with no runtime behaviour change. #334 is a lockfile-only bump. They are independent; merge in either order. Acceptance: all CI checks green, owner merges.

**Dependabot triage recipe** (for future alerts):
1. List open alerts: `gh api 'repos/hherb/primer/dependabot/alerts?state=open' --jq '.[] | {n:.number, sev:.security_advisory.severity, pkg:.dependency.package.name, fix:.security_vulnerability.first_patched_version.identifier}'`.
2. Check reachability with `cargo tree -i <pkg> --workspace --all-features --target all`. Empty output means a lockfile-only entry, so the alert can't be exploited.
3. Fix with `cargo update -p <pkg> --precise <fix>` on its own branch off `main`.
4. Verify with `cargo check --workspace --all-targets --locked`.

### 1. Sweep + inline-test detector: re-run EVERY session (clean five sessions running)
Commands are in the resume block. Watch-list: `primer-classifier/src/llm.rs` (441) and `primer-extractor/src/llm.rs` (454) carry inline test modules and sit near the 500 threshold.

### 2. Production-code splits (the open, owner-approved lane)
Remaining oversized **production** files (post-#333 sweep), lowest-risk first. **All remaining ones are feature-gated → dual-verify.**
- **`primer-gui/src/commands/voice.rs` (559, `speech`-gated)**, recommended next. Baseline both `cargo test -p primer-gui` and `cargo test -p primer-gui --features speech`, plus `cargo clippy -p primer-gui --features speech --all-targets -- -D warnings`. Check its shape first: N Tauri commands (mechanical) or one big function (needs stage extraction + stage tests, like #333).
- **`primer-inference/src/qnn/genie/real.rs` (566, `qnn`-gated)**: the default build covers the host-mock path, and `--features qnn` covers the real arm (`cargo clippy -p primer-inference --features qnn --all-targets -- -D warnings`). The file is Android-only at runtime, so the host can only *compile* the real arm.
- **`primer-speech/src/voice_loop/state_machine/inner.rs` (506)**: already inside a split dir; `mocks.rs` reaches siblings via `super::`. Only 6 lines over. Likely one big `run_loop_inner` function, so it would be a stage extraction, not mechanical. Low value for the risk; consider leaving it.
- **`primer-speech/src/macos/{tts.rs 668, stt.rs 504}`** (`macos-native`-gated; this host is macOS so the feature build works here).
- Hardest: `primer-cli/src/main.rs` (1357, heavily `cfg(feature)`-gated, needs the per-feature clippy+test matrix).
- **Off the list:** `consts.rs` (#322), `prompt_builder.rs` (#321), `dialogue_manager/turn.rs` (#320), `state_machine.rs` (#318), `schema.rs` (#328), `config/types.rs` (#329), `wiring.rs` (#333).
- The >500-line test-support files (`store/tests/session_tests.rs` 2442, `state_machine/mocks.rs` 1381, `dialogue_manager/tests/turn_tests.rs` 1178, `config/tests.rs` 1086, `dialogue_manager/tests/background_tests.rs` 777, `kb-load/tests/common/mod.rs` 677, `dialogue_manager/test_support.rs` 655, `store/tests/learner_tests.rs` 607) remain lower-value than production files.

No re-ask needed: this lane is owner-approved.

### 3. Issue #330: rustdoc link cleanup + CI guard (host-completable, a good break from splits)
Acceptance: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items` exits 0, and that command is a step in `.github/workflows/ci.yml`. `primer-gui` holds about 40% of the 43 links. **Fix by escaping the link text, not by making modules `pub`** (that would defeat the façade pattern). #333 may have shifted the `primer-gui` count slightly: the new modules' docs use plain backticks, not intra-doc links, so it should be no worse, but re-count.

### Carried / owner-or-hardware-gated (none host-completable autonomously)
- **#260**: Android-voice on-device acceptance (RedMagic 11 Pro + mic + quiet room).
- **#192**: manual macOS-native STT + injected non-AVSpeech TTS audio path.
- **#170 Stage B / E / F**: Supertonic voice-mode TTS, in-loop A/B numbers, Hindi preview→stable. The OpenRAIL-M clause (e) disclosure must ship before any default Supertonic flip.
- **#166 item #1**: owner-run WhisperStream reuse smoke (command at the bottom).
- **#135**: glib 0.18.5 → 0.20+ (blocked on Tauri 3).
- **Branch protection**: make `cargo test (default features)` a required check on `main` (owner GitHub-settings call).
- QNN stable-token-across-reboots gate; NPU pedagogy/answer-quality tuning; latency-routing calibration.

## Open decisions / risks

- **PR #333 open, awaiting owner review.** The pub-surface gate is weak for this split, so review leans on the line-diff audit (in the PR body) and the 10 new stage tests. Worth a careful read of `stores.rs`, which holds the locale-mismatch hard-fail and the resume re-tag logic.
- **Dependabot: triaged this session.** The only open alert (#6, `quinn-proto`, high) is a lockfile-only entry that is never compiled; PR #334 bumps it. Once #334 merges, zero alerts remain open. The dismissed #2 (glib, `tolerable_risk`) stays tied to #135 / Tauri 3.
- **The `github` MCP server failed to connect** (bad Authorization header). The `gh` CLI worked fine for PRs and issues.
- **Build times (warm):** `cargo test -p primer-gui` ~25 s, workspace clippy ~1–4 min, full workspace test ~7 min. Run ONE cargo pass at a time. **Don't edit sources while a baseline `cargo test` is still compiling**: if rustc reaches the crate after your edit, the "baseline" silently tests the new code.

## Patterns to reuse, not reinvent

### Split recipe
1. **Baseline:** `cargo test -p <crate>` (plus any `--features` that gate tests) green BEFORE touching anything; record the pass count.
2. **Check the shape:** N independent top-level items (mechanical; pub-surface + line diff prove it) or one big function (stage extraction; see #333 below).
3. **Move:** `git mv foo.rs foo/mod.rs`, then write the siblings. With a pristine copy of the original outside the repo (`git show main:src/… > $SCRATCH/orig.rs`), you can `sed -n 'A,Bp'` ranges out of it for byte-exact moves.
4. **Visibility across the new boundary**, six cases seen so far:
   1. already-`pub mod` blocks: no work;
   2. a parent that globs you: `pub use sub::*;` façade;
   3. private items a test child reaches: keep them in `mod.rs`;
   4. a `pub(super)` method that parent-level tests call: `pub(in crate::<path>)`;
   5. a sibling-moved helper a test reaches via `use super::*`: `pub(super)` + `#[cfg(test)] use sub::name;` (plain `use`, E0364 otherwise);
   6. mixed `pub`/`pub(crate)` façade: re-export each at its own visibility (E0365).

   Children reach ancestors' private items (e.g. `LocaleStrategy`) for free.
5. **Fix paths:** prefer absolute `crate::…` imports in new submodules. Watch for unused imports (`-D warnings`).
6. **Verify:** pub-surface diff; the order-insensitive substantive-line diff (snippet below, audit every line); crate count matches baseline; feature-combo checks if cfg arms exist; workspace test; clippy `-D warnings`; fmt `--check` (run fmt AFTER the split).

### Stage extraction (#333), for single-big-function files
Extract each `// ─── stage ───` block into a `pub(super)` fn that takes `&GuiConfig`-style inputs and returns the locals the next stage needs (a tuple is fine). Keep the orchestrator calling them in the **original order** so the side-effect order is provably unchanged. Then add a `stage_tests.rs` sibling that tests each stage directly. The extraction is what makes those tests possible, and they are what the review relies on instead of the pub-surface diff.

### Substantive-line diff
```bash
strip() { grep -vE '^\s*//' | grep -vE '^\s*$' | grep -vE '^\s*(pub |pub\(crate\) |pub\(super\) )?(use |mod ) ?' | sed -E 's/^\s+//'; }
strip < $SCRATCH/orig.rs | sort > $SCRATCH/old.txt
cat <new files> | strip | sort > $SCRATCH/new.txt
diff $SCRATCH/old.txt $SCRATCH/new.txt   # audit EVERY line; mechanical splits → only façade lines
```
The `sed` de-indent matters for stage extraction: code moved one indentation level would otherwise show up as a diff on every line.

### Other habits
- **Doc-drift hunt:** grep the moved file name AND its key symbols across README/ROADMAP/CLAUDE/docs. In CLAUDE.md, `file.rs::symbol` references are the usual casualties.
- **Cite the PR number in CLAUDE.md** as a follow-up commit, once the PR exists.
- **Read `NEXT_SESSION.md` AFTER `git pull`.** This session first read a stale copy, before pulling 4 commits, and nearly followed an outdated recommendation.
- **Shell and cargo habits:**
  - A trailing `grep` that matches nothing returns exit 1; check the `test result:` lines, not the exit code.
  - Branch each PR off `main`.
  - Run long cargo passes with `run_in_background: true`.
  - To wait for a running cargo, use an `until ! pgrep -f 'cargo test' …; do sleep 2; done` loop inside a Bash call, not a bare `sleep`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git checkout main && git pull --ff-only && git log --oneline -4 && gh pr list --state open
# Then re-read NEXT_SESSION.md (it may have changed in the pull). Two PRs open at close: #333 (wiring split), #334 (quinn-proto bump).
SCRATCH="${TMPDIR:-/tmp}/primer-scratch"; mkdir -p "$SCRATCH"   # or the session's scratchpad dir

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done
# Empty inline output = no cheap pick → next production split or issue #330.

# === Recommended next split: primer-gui/src/commands/voice.rs (559, speech-gated). Baselines FIRST: ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-gui 2>&1 | grep 'test result: ok'                     # expect 217 after #333 merges
~/.cargo/bin/cargo test -p primer-gui --features speech 2>&1 | grep 'test result: ok'    # record this count too
~/.cargo/bin/cargo clippy -p primer-gui --features speech --all-targets -- -D warnings

# === Standard workspace gate (one cargo pass at a time) ===
~/.cargo/bin/cargo test --workspace 2>&1 | tee $SCRATCH/ws.log | tail -3
grep -cE 'test result: ok' $SCRATCH/ws.log            # expect 51
grep -E 'test result: FAILED|^error' $SCRATCH/ws.log  # expect empty
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo fmt --all -- --check

# === Pub-surface diff (repo-root-relative git path; note `mod` AND `type`) ===
git show main:src/crates/<path>.rs | grep -oE 'pub (struct|enum|fn|const|async fn|trait|type|mod) [A-Za-z_0-9]+' | sort -u > $SCRATCH/old-pub.txt
cat <new files> | grep -oE 'pub (struct|enum|fn|const|async fn|trait|type|mod) [A-Za-z_0-9]+' | sort -u > $SCRATCH/new-pub.txt
diff $SCRATCH/old-pub.txt $SCRATCH/new-pub.txt

# === Issue #330: rustdoc link count ===
~/.cargo/bin/cargo doc --workspace --no-deps --document-private-items 2>&1 \
  | grep -E '^warning: (unresolved link|public documentation)' -A1 | grep -oE 'crates/[a-z0-9-]+/src' | sort | uniq -c | sort -rn

# === Carried: owner-run the #166 reuse smoke (needs a model + two 16 kHz mono WAVs) ===
PRIMER_WHISPER_MODEL=/path/to/ggml-small.en.bin \
PRIMER_WHISPER_AUDIO_A=/path/to/utterance_a.wav \
PRIMER_WHISPER_AUDIO_B=/path/to/utterance_b.wav \
  ~/.cargo/bin/cargo test -p primer-speech --features whisper \
  --test whisper_stream_reuse -- --ignored --nocapture
```

## Reporting back

- **PR #333 (`wiring.rs` split):** 591 lines became `{mod 212, stores 185, backend_params 173, subsystems 145}` + new `stage_tests.rs` (236, 10 tests). This was a stage extraction from one ~395-line function, not a mechanical split, so it was verified with an audited line diff plus per-stage tests. 207 → 217 tests, 0 failed; workspace green; clippy/fmt clean.
- **PR #334 (Dependabot #6):** `quinn-proto` lockfile bump. Not exploitable, since it is never compiled (only reachable via reqwest's unused `http3` feature). Once it merges, zero open alerts remain.
- **All prior-brief PRs merged between sessions.** The inline-test detector has been clean five sessions running.
- **Next:** `commands/voice.rs` (speech-gated, dual-verify), or issue #330 (rustdoc links + CI guard).
- The GUI is a full app, not a scaffold.
