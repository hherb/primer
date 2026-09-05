# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-09-04 11:41 UTC. On branch `refactor/split-gui-config-types` at `32fe5a8`. **Two new PRs open against `main`, both branched independently off `main` at `a062791`: #328 (storage `schema.rs` split) and #329 (GUI `config/types.rs` split).** One new issue filed: **#330** (43 broken rustdoc intra-doc links workspace-wide). `main` is at `a062791` — the prior session's PR #322 (consts split) merged between sessions, as did #327 (parlor research doc).

This session ran the standard sweep FIRST: the oversized-file sweep found the expected list and the inline-test detector was **clean again — fourth consecutive clean run**. So the session went straight to the owner-approved production-split lane and shipped **two** splits: the recommended pick plus one more.

## What we shipped this session

### PR #328 (branch `refactor/split-storage-schema`) — `primer-storage/src/schema.rs`, 623 lines
Commits **`d008638`** (the split) + **`fc02798`** (cite the PR number in CLAUDE.md).

`schema.rs` held four concerns in one file: the version constant, the v1 baseline DDL, the seven-step additive migration chain (with each version's DDL constants interleaved), and the lookup-table validate-and-seed pass. New layout — largest file 104 lines:

| File | Lines | Holds |
|---|---:|---|
| `schema/mod.rs` | 104 | `USER_VERSION` + `SCHEMA_SQL` + flat re-export façade |
| `schema/migrations/mod.rs` | 49 | chain doc, decls + re-exports, shared `column_exists` / `table_exists` |
| `schema/migrations/v2.rs` … `v8.rs` | 41–102 | one file per version step |
| `schema/lookup.rs` | 80 | `validate_and_seed_lookup` |
| `schema/v4_tests.rs` | 335 | **untouched** |

- **One file per version step** keeps each version's DDL next to the `apply_vN_migrations` that runs it — the existing repo rule that one-shot schema SQL reads best next to its logic. Section-banner comments (`// ─── v4 schema strings ───`) became `//!` module docs, so the substantive rationale (v4's JSON-in-TEXT note, v5's cascade-design note) is now each file's own header.
- Adding v9 is now: add `migrations/v9.rs`, one `mod` + one `pub(crate) use` line, call from the open path, bump `USER_VERSION`.
- Declared visibilities preserved exactly (`pub` ×4, `pub(crate)` ×6), so every `crate::schema::<name>` path in `store/mod.rs`, `catalog.rs`, `store/tests/session_tests.rs` is unchanged and `v4_tests.rs` (which reaches everything by bare name via `use super::*`) needed zero churn.

### PR #329 (branch `refactor/split-gui-config-types`) — `primer-gui/src/config/types.rs`, 539 lines
Commits **`f3ab529`** (the split) + **`32fe5a8`** (cite the PR number in CLAUDE.md).

Grouped by settings area — the same grouping the settings modal renders as collapsible sections:

| File | Lines | Holds |
|---|---:|---|
| `types/mod.rs` | 44 | `GuiConfig` umbrella + flat `pub use <submodule>::*;` façade |
| `types/backend.rs` | 198 | `BackendConfig`, `ApiKeySource`, `ApiKeySourceView`, `ApiKeyUpdate` |
| `types/speech.rs` | 146 | `SpeechBackend`, `SttBackend`, `TtsBackend`, `SpeechSettings`, `SpeechLocaleOverride` |
| `types/sections.rs` | 107 | `LearnerConfig`, `VocabConfig`, `BreakConfig`, `PersistenceConfig`, `UiConfig`, `DiagnosticsConfig` |
| `types/subsystems.rs` | 105 | `SubsystemConfig`, `EmbedderConfig`, the cfg-gated `default_embedder_kind()` (still private) |

- `config/mod.rs` already did `pub use types::*;` and `types/mod.rs` now globs its own submodules, so every `crate::config::Foo` path — and the `use super::*` in the 1086-line `config/tests.rs` — resolves as before. Serde attributes travel with their types, so **the on-disk `gui-config.json` shape is untouched**.

### Issue #330 — 43 broken rustdoc intra-doc links across 8 crates
Filed per CLAUDE.md's *fix it or file it* principle. Surfaced while verifying #329: the split added zero rustdoc errors, but `primer-gui` was already 17 deep on `main`. Breakdown (default features, so it's a floor — feature-gated code isn't covered): `primer-gui` 17, `primer-pedagogy` 8, `primer-knowledge` 7, `primer-engine` 5, `primer-qnn-sys` 2, `primer-core` 2, `primer-inference` 1, `primer-cli` 1. Two failure modes, with a full worked list of the `primer-gui` 17 in the issue. `primer-storage` is clean. `cargo doc` is **not** a CI gate today; the issue proposes landing a guard after the fixes.

### Verification (both PRs)
- **#328:** `cargo test -p primer-storage` → **154 / 154**, exact baseline match. Pub surface byte-identical (4 symbols) **and** `pub(crate)` surface byte-identical (6 symbols). `cargo doc -p primer-storage --document-private-items` clean under `RUSTDOCFLAGS=-D warnings`.
- **#329:** `cargo test -p primer-gui` → **207 / 207**, exact baseline match. Pub surface byte-identical (23 symbols). `cargo check -p primer-gui --no-default-features` green — exercises the `#[cfg(not(feature = "embedding"))]` arm of `default_embedder_kind()` the default build configures out.
- **Both:** an order-insensitive line diff of all substantive (non-comment, non-import) lines old-vs-new is empty except for the re-export façade lines deliberately added — proof no DDL, migration, or field line was lost, silently altered, or duplicated. This check is a strict superset of the pub-surface diff and is worth keeping in the recipe.
- **Both:** `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --all -- --check` clean; `cargo test --workspace` green (**51 × `test result: ok`, 0 failed**) — run separately per branch.
- CI at close: #328 had 6 checks passing, 4 still pending; #329 had 1 passing, 9 pending. **Nothing failing.**

### Docs
- **CLAUDE.md** — `primer-storage` bullet gains the schema-split description; the "Schema migrations" bullet's *Adding a new schema version* instruction points at the new layout; the hybrid-retrieval bullet points at `config/types/subsystems.rs`; the `primer-gui` `config`-module note records the nested split.
- **docs/devel/05-storage-and-sessions.md** — 12 stale `schema.rs` links, two code-fence paths, and the "Add a schema migration" recipe all updated; every relative link in the file verified to resolve programmatically.
- **docs/devel/android-test-apk-build.md** — Step 2's `default_embedder_kind()` path corrected.
- Two pre-existing doc nits fixed inline: the storage recipe misquoted CLAUDE.md's "Schema is at version 8." as "user_version 8", and CLAUDE.md called the migration template "the existing v2..v7 chain" when the chain reaches v8.
- **README.md and ROADMAP.md deliberately NOT changed** — both describe capabilities and schema versions, not file layout. Neither split alters a capability, a schema version, or any user-facing behaviour. Grep-verified: neither file references `schema.rs`, `config/types.rs`, or any `primer-*/src` path.
- Frozen `docs/superpowers/plans/` and `docs/handoffs/` copies left as-is by convention. One plan (`2026-07-01-android-generic-api-test-apk.md:62`) carries a now-stale `grep … config/types.rs` line; it is a historical record of what was run at the time, and the live devel doc is the one a future session follows.

## What's next (concrete acceptance criteria)

### 0. PRs #328 and #329 — owner review/merge
Both are pure refactors, no runtime behaviour change, independent of each other (different crates, both off `main` — merge in either order, no rebase needed). Acceptance: all checks green, owner merges.

### CHEAP TEST EXTRACTIONS: detector CLEAN four sessions running — but re-run it EVERY session
Commands in the resume block. Watch-list unchanged: `primer-classifier/src/llm.rs` (~460) and `primer-extractor/src/llm.rs` (~470) both carry inline test modules and sit near the threshold.

### Production-code splits — the open, owner-approved lane

**Read this before picking:** the remaining files are NOT all the same shape. Five of the eight splits so far (#318, #320, #321, #322, #328, #329) were *mechanical* — a file that was already N independent top-level items, split one-file-per-group, provably identical by the pub-surface + line diffs. The pub-surface-diff gate is what makes those safe to merge on a quick read. **That gate does not protect a split of a single large function.** Sort the remaining list accordingly:

**Mechanical (same shape as everything shipped so far):**
- **`primer-inference/src/qnn/genie/real.rs` (566, qnn-gated)** — dual-verify: needs `--features qnn` for the real arm plus the default build for the host-mock path.
- **`primer-gui/src/commands/voice.rs` (559, speech-gated)** — dual-verify with `--features primer-gui/speech`.
- **`primer-speech/src/voice_loop/state_machine/inner.rs` (506)** — already inside a split directory; check what `mocks.rs` reaches via `super::`.
- **`primer-speech/src/macos/{tts.rs 668, stt.rs 504}`** (macos-native-gated; needs a macOS host for the feature build — this host is macOS, so it is doable here).

**NOT mechanical — needs its own plan:**
- **`primer-gui/src/wiring.rs` (591)** — looks attractive (zero feature gates, `cargo test -p primer-gui` guard) and the previous brief listed it as a near-term pick, but it is **one 395-line function**: `build_with_strategy` spans lines 99–493, with 11 `// ─── stage ───` banners and locals threaded from stage to stage. Everything else in the file is ~90 lines of small helpers. Splitting it means *extracting stage functions and threading parameters* — a logic refactor where the pub-surface diff is vacuous (all movement is inside one private fn) and only `cargo test -p primer-gui` (207 tests) stands behind it. Worth doing, but give it a written plan and per-stage reasoning rather than treating it as the next mechanical pick.
- **`primer-cli/src/main.rs` (1357)** — hardest; heavily `cfg(feature)`-gated, needs the per-feature clippy+test matrix.
### Production-code splits — the open, owner-approved lane (pick the next one, lowest-risk first)
Remaining oversized **production** (non-test) files after #322 (post-split sweep):
- **`primer-gui/src/wiring.rs` (591)** / **`primer-gui/src/config/types.rs` (539)** — **recommended next pick.** No feature gates, GUI-heavy; `cargo test -p primer-gui` + workspace guard.
- `primer-storage/src/schema.rs` — **NO LONGER on the list** (done in #328: `schema/mod.rs` + `schema/migrations/v2..v8.rs` + `schema/lookup.rs`, all ≤ 104 lines).
- **`primer-inference/src/qnn/genie/real.rs` (566, qnn-gated)** / **`primer-gui/src/commands/voice.rs` (559, speech-gated)** / **`primer-speech/src/voice_loop/state_machine/inner.rs` (506)** / **`primer-speech/src/macos/{tts.rs 668, stt.rs 504}` (macos-native-gated)** — feature-gated (dual-verify; the macos ones need a macOS host for the feature build).
- Hardest: `primer-cli/src/main.rs` (1357, heavily `cfg(feature)`-gated — needs the per-feature clippy+test matrix).
- `consts.rs` — **NO LONGER on the list** (fixed by #322). `prompt_builder.rs` — off since #321. `dialogue_manager/turn.rs` — off since #320.

Off the list: `consts.rs` (#322), `prompt_builder.rs` (#321), `dialogue_manager/turn.rs` (#320), `schema.rs` (#328), `config/types.rs` (#329).

The >500 test-support files (`store/tests/session_tests.rs` 2442, `state_machine/mocks.rs` 1381, `dialogue_manager/tests/turn_tests.rs` 1178, `config/tests.rs` 1086, `dialogue_manager/tests/background_tests.rs` 777, `kb-load/tests/common/mod.rs` 677, `dialogue_manager/test_support.rs` 655, `store/tests/learner_tests.rs` 607) remain lower-value than the production files.

**No re-ask needed — the production-split lane is owner-approved.** Only re-confirm if changing lanes back to docs/maintenance.

### Issue #330 — rustdoc link cleanup (new, host-completable)
A genuinely good next task if you want a break from splits: fix the 43 links, then land the `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items` CI guard. Acceptance: that command exits 0, and the guard is in `.github/workflows/ci.yml`. Split per-crate if you like — `primer-gui` is 40% of the total alone. **Fix mode (a) by escaping the link, not by making the module `pub`** — that would defeat the façade pattern the splits deliberately use.

### The proven split recipe (seventh and eighth clean runs this session)
1. Baseline: `cargo test -p <crate>` (with the right `--features` if tests are gated) green BEFORE touching anything; record the pass count.
2. Read the whole file; map natural responsibility boundaries. **Check the shape first** — N independent top-level items (mechanical) vs one large function (not mechanical; see above).
3. Convert `foo.rs` → `foo/mod.rs` + siblings via `git mv foo.rs foo/mod.rs`, then write the siblings. Keep a pristine copy of the original outside the repo and `sed -n 'A,Bp'` the ranges out of *that*, so extraction is byte-exact by construction.
4. **Visibility across the new module boundary — six cases seen so far:**
   - Already-`pub mod` blocks (`consts.rs`, #322): trivial — one file per block, no re-exports, `tests.rs` untouched.
   - A **glob façade** where the parent already globbed you (`config/types`, #329): `pub use <submod>::*;` in the new `mod.rs` and every downstream path is unchanged for free.
   - Private helpers/consts a *test child of the parent* reaches (`FALLBACK_LINE`, #318; `english_pack`, #321): keep them IN `mod.rs`.
   - A `pub(super)` method that *parent-level tests* call directly (`build_turn_prompt`, #320): re-declare as `pub(in crate::<parent-path>)`.
   - A private helper a *test child* reaches by bare name via `use super::*` but which now lives in a *sibling* submodule (`is_factual_question*`, #321): mark it `pub(super)` and add a `#[cfg(test)] use <submod>::<name>;` (PLAIN, not `pub(crate) use` — E0364) in `mod.rs`.
   - Mixed `pub` / `pub(crate)` items re-exported through one façade (`schema`, #328): re-export each at its *own* visibility — `pub use` of a `pub(crate)` item is **E0365**. Grandchildren reach an ancestor's private helper for free (`migrations/v2.rs` → `use super::column_exists;` where `column_exists` is plain-private in `migrations/mod.rs`); no `pub(super)` needed.
5. Fix moved relative paths: prefer absolute `crate::…` imports (or `super::` for a re-exported sibling) in the new submodules. (`mod.rs` itself keeps plain `super::…` — its depth is unchanged.) Check per-range which imports each new file actually needs (`grep -c PathBuf`, `grep -oE 'primer_[a-z_]+::[A-Za-z_:]+'`) — an unused import is a `-D warnings` failure.
6. Verify: pub-surface diff empty, **plus** the order-insensitive substantive-line diff (below), crate suite count matches baseline, feature-combos check if cfg arms exist, full-workspace `cargo test`, clippy `-D warnings`, fmt `--check` (run fmt AFTER the split).

## Open decisions / risks

- **PRs #328 and #329 open, awaiting owner review/merge.** Both pure refactors, no runtime behaviour change; CI pending-but-nothing-failing at close.
- **The production-split lane stays open and owner-approved.** Recommended next mechanical pick: `primer-inference/src/qnn/genie/real.rs` (566) or `primer-gui/src/commands/voice.rs` (559) — both feature-gated, so dual-verify. **Do not treat `primer-gui/src/wiring.rs` as a mechanical pick** (see above); the previous brief listed it as near-term without noting it is a single 395-line function.
- **The inline-test detector was clean this session (fourth consecutive).** Between-sessions PRs can still push near-threshold files over 500. Both sweeps are cheap — run them before picking work.
- **Two other PRs are open that this session did not touch:** #323 (dependabot `serde_with` 3.20→3.21) and #324 (draft OmniVoice TTS-suitability doc). GitHub also reports 2 dependabot vulnerability alerts on `main` (1 high, 1 moderate) on every push — worth an owner look; not triaged this session.
- **Machine load / build times:** deps warm — `cargo test -p primer-storage` seconds, `cargo test -p primer-gui` ~16 s, workspace clippy ~4 m, full workspace test ~7 m, `cargo doc --workspace` ~2 m. Run ONE cargo pass at a time; don't run fmt (source-modifying) while clippy is mid-flight on the same crate.
- **The `github` and `greptile` MCP servers failed to connect this session** (bad Authorization header / 403). `gh` CLI worked fine throughout and is the reliable path for issues + PRs.
- **PR #322 open, awaiting owner review/merge.** Pure refactor, no runtime behaviour change; CI pending at close.
- **The production-split lane stays open and owner-approved.** Recommended next: `primer-gui/src/wiring.rs` (591) / `primer-gui/src/config/types.rs` (539) — no feature gates, `cargo test -p primer-gui` + workspace guard. (`schema.rs` was the prior recommendation; done in #328.)
- **The inline-test detector was clean this session (third consecutive).** Between-sessions PRs can still push near-threshold files (`primer-classifier/src/llm.rs` ~460, `primer-extractor/src/llm.rs` ~470) over 500. The sweep + detector are cheap — run both before picking work.
- **Machine load / build times:** deps warm — `cargo test -p primer-core` seconds, workspace clippy ~4m, full workspace test ~7 min. Cold-start budget ~35 min for the first cargo pass. Run ONE cargo pass at a time; don't run fmt (source-modifying) while clippy is mid-flight on the same crate.

## Patterns to reuse, not reinvent

- **The 6-step split recipe above.** Two new cases this session: (a) a mixed-visibility façade needs per-visibility re-exports (E0365 if you `pub use` a `pub(crate)` item); (b) when the *parent* module already glob-re-exports you, a glob façade in the new `mod.rs` makes every downstream path free.
- **The order-insensitive substantive-line diff is the strongest cheap gate** — strictly stronger than the pub-surface diff, since it catches a silently-altered SQL string or struct field that keeps the same signature:
  ```bash
  strip() { grep -vE '^\s*//' | grep -vE '^\s*$' | grep -vE '^\s*(pub |pub\(crate\) )?(use |mod ) ?'; }
  strip < /path/to/pristine-original.rs | sort > /tmp/old.txt
  cat <new files> | strip | sort > /tmp/new.txt
  diff /tmp/old.txt /tmp/new.txt   # only the re-export façade lines you added should appear as `>`
  ```
  Expect **zero `<` lines**. A few `>` lines are your façade (note the filter is imperfect on `pub use` — read them, don't assume).
- **Pub-surface diff must include `pub mod` AND `pub type`**. Full regex: `pub (struct|enum|fn|const|async fn|trait|type|mod) [A-Za-z_0-9]+`. Watch for doc-comment false positives. Also diff the `pub\(crate\)` surface separately when the module is crate-private — the `pub` regex alone proves nothing there.
- **Doc-drift hunt recipe:** after a split, grep the moved file's name AND its key symbols across README/ROADMAP/CLAUDE/docs. Then **programmatically validate every relative markdown link** in any doc you edited:
  ```python
  for m in re.finditer(r'\[([^\]]+)\]\((\.\.[^)]+)\)', doc.read_text()):
      assert (doc.parent / m.group(2)).resolve().exists()
  ```
  This session that turned up 12 stale `schema.rs` links in one devel doc that a plain grep-and-eyeball would have partly missed.
- **Two PRs in one session works fine** — commit the second split on whatever branch you're on, then `git checkout -b <new> main && git cherry-pick <sha>`, then `git reset --hard origin/<first-branch>` to drop it from the first. Keeps both PRs independent off `main` per the standing rule. Verify afterwards that each branch's CLAUDE.md carries only its own paragraph.
- **Cite the PR number in CLAUDE.md as a follow-up commit** on the same branch — you can't know the number until the PR exists, and a second commit is cheaper than a force-push.
- **Use ABSOLUTE paths for shell tools** — Bash cwd persists between calls but is easy to lose track of; a failed `cd` in an `&&` chain silently runs the rest from the *previous* directory.
- **A grep final pipe stage returns exit 1 on zero matches** — check the `test result:` / `Finished` lines, not the pipeline exit code.
- **Branch each PR off `main`, not off the previous branch.**
- **Run long cargo passes with `run_in_background: true`**; the harness notifies on completion. Do NOT chain `sleep`; the harness blocks foreground `sleep`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git fetch && git log --oneline -4 && gh pr list --state open
# Two PRs open at close from this session: #328 (schema.rs split), #329 (config/types.rs split).
# Plus #323 (dependabot) and #324 (draft doc), untouched. After #328/#329 merge, start fresh off main.

# === Standard workspace gate (run from src/ if you touch .rs) — one cargo pass at a time, grep twice ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test --workspace 2>&1 | tee /tmp/ws.log | tail -3
grep -cE 'test result: ok' /tmp/ws.log            # expect 51
grep -E 'test result: FAILED|^error' /tmp/ws.log  # expect empty
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo fmt --all -- --check

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done
# Empty inline output = no cheap pick. Next host-actionable work = next production split, or issue #330.

# === Recommended next mechanical split: qnn/genie/real.rs (566) — dual-verify, baseline FIRST ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-inference 2>&1 | grep 'test result: ok'                    # default (host-mock) baseline
~/.cargo/bin/cargo clippy -p primer-inference --features qnn --all-targets -- -D warnings    # the gated arm

# === Issue #330 — rustdoc link cleanup + CI guard ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo doc --workspace --no-deps --document-private-items 2>&1 \
  | grep -E '^warning: (unresolved link|public documentation)' -A1 | grep -oE 'crates/[a-z0-9-]+/src' | sort | uniq -c | sort -rn
# expect 43 across 8 crates today; goal is 0, then land RUSTDOCFLAGS="-D warnings" cargo doc … in ci.yml
# === Recommended next split: primer-gui/src/wiring.rs (591) or config/types.rs (539). Baseline FIRST: ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-gui 2>&1 | grep 'test result: ok'   # record pass count BEFORE splitting
# (primer-storage/src/schema.rs was the prior recommendation — done in #328.)
# Split by responsibility, keep the external pub surface stable, and prefer a GLOB re-export in the
# facade over a name list so a new submodule needs no second edit site. Then re-verify same count + pub-surface
# diff + clippy + fmt + workspace.

# === Behaviour-preserving pub-surface diff (repo-root-relative git path; note `mod` AND `type`) ===
git show main:src/crates/<path>.rs | grep -oE 'pub (struct|enum|fn|const|async fn|trait|type|mod) [A-Za-z_0-9]+' | sort -u > /tmp/old-pub.txt
cat <new submodule files> | grep -oE 'pub (struct|enum|fn|const|async fn|trait|type|mod) [A-Za-z_0-9]+' | sort -u > /tmp/new-pub.txt
diff /tmp/old-pub.txt /tmp/new-pub.txt   # empty = identical external pub surface
# ...and repeat with 'pub\(crate\) (struct|enum|fn|...)' when the module is crate-private.

# === Carried: owner-run the #166 reuse smoke (needs a model + two 16 kHz mono WAVs) ===
PRIMER_WHISPER_MODEL=/path/to/ggml-small.en.bin \
PRIMER_WHISPER_AUDIO_A=/path/to/utterance_a.wav \
PRIMER_WHISPER_AUDIO_B=/path/to/utterance_b.wav \
  ~/.cargo/bin/cargo test -p primer-speech --features whisper \
  --test whisper_stream_reuse -- --ignored --nocapture
```

## Carried / owner-or-hardware-gated (none host-completable autonomously)

- **#260** — Android-voice on-device acceptance (RedMagic 11 Pro + mic + quiet room).
- **#192** — manual macOS-native STT + injected non-AVSpeech TTS audio path (mic + macOS build).
- **#170 Stage B / E / F** — Supertonic voice-mode TTS + in-loop A/B numbers + Hindi preview→stable (OpenRAIL-M clause (e) disclosure must ship before any default Supertonic flip).
- **#166 item #1** — owner-run WhisperStream reuse smoke (model + two 16 kHz WAVs; command above).
- **#135** — glib 0.18.5 → 0.20+ (blocked on Tauri 3).
- **Branch protection** — wire `cargo test (default features)` as a required status check on `main` (owner GitHub-settings call; still outstanding).
- **Dependabot alerts** — 2 open on `main` (1 high, 1 moderate); PR #323 may or may not address them. Owner triage.
- QNN stable-token-across-reboots gate; NPU pedagogy/answer-quality tuning; latency-routing calibration.

## Reporting back

- **PR #328 (`schema.rs` split):** the recommended pick from the prior brief. 623 → `{mod 104, backend-of-chain: migrations/mod 49, v2 84, v3 70, v4 102, v5 94, v6 52, v7 41, v8 41, lookup 80}`. 154/154 crate tests (baseline match), pub AND `pub(crate)` surfaces byte-identical, zero churn in the 335-line `v4_tests.rs`, rustdoc clean under `-D warnings`, workspace clippy/fmt/test all green.
- **PR #329 (`config/types.rs` split):** a bonus second split from the same lane. 539 → `{mod 44, backend 198, speech 146, sections 107, subsystems 105}`. 207/207 crate tests (baseline match), 23-symbol pub surface byte-identical, on-disk `gui-config.json` shape untouched, both `embedding` feature arms compile-checked.
- **Issue #330 filed** — 43 broken rustdoc intra-doc links across 8 crates, with a full worked list for `primer-gui` and a suggested per-crate resolution + CI guard. Confirmed real, out of scope for two refactor PRs, so filed rather than fixed (per *fix it or file it*).
- **A shape warning for the next session:** `primer-gui/src/wiring.rs` (591) is **not** a mechanical split — it is one 395-line function. The prior brief listed it as a near-term pick without that caveat. Give it a plan.
- **The inline-test detector came up clean for the fourth consecutive session.**
- **PR #322 (consts.rs split):** the recommended pick from the prior brief. 562 → `{mod 25, speech 199, retrieval 100, router 46, vocab 32, prompt_budget 32, inference 23, retry 20, pedagogy 20, reasoning 16, qnn 14, learner 14, break_suggest 12}` — 77-symbol external pub surface byte-identical, 176/176 crate tests (baseline match), zero churn in the 40-line `tests.rs` (it reaches submodules by name), workspace clippy/fmt clean, workspace suite green (51 ok). The easiest split yet — every area was already a `pub mod` block, so no visibility work at all.
- **Prior session's PR #321 merged between sessions** — the inline-test detector came up clean for the third consecutive session.
- **The production-split lane is open and owner-approved** — next pick `primer-gui/src/wiring.rs` (591) or `primer-gui/src/config/types.rs` (539) without re-asking. (`schema.rs` done in #328.)
- The GUI is a full app, not a scaffold.
