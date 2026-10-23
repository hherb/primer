# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-10-23 (UTC). **Two PRs are open against `main`; they are independent and can merge in either order:**

| PR | Branch | What | Closes |
|---|---|---|---|
| **#360** | `fix/private-primer-dir-358` | `~/.primer/` learner data kept owner-only (`0700`) | #358 |
| **#361** | `fix/stage-test-gaps-340` | primer-gui wiring stage-test gaps closed (this brief rides on it) | #340 |

`main` is at `a076f7c` (#359). Since the last brief, **#359 merged** and **#341 is confirmed CLOSED**.

## What we shipped this session

### #360 (`6c35c3c`): closes #358, a children's-data privacy fix
**Problem:**
- Only `gui-config.json` (`0600`) and, since #357, `gui.log` (`0600`) were restricted.
- `~/.primer/` itself was `0755` and the session DBs `0644`, so another OS login on a shared Mac (every account is in `staff`) could read a child's full transcripts.

**Fix:**
- New `primer_engine::create_private_dir_all`: on Unix, every directory it newly creates is `0700`. Existing components are untouched, so an explicit `--session-db ~/Documents/x.db` never chmods `~/Documents`.
- New `primer_engine::ensure_session_db_dir(session_path, home)`:
  - replaces the duplicated CLI (`main.rs`) and GUI (`wiring/stores.rs`) parent-dir block;
  - also tightens an existing `~/.primer/` to `0700` (the parent protects the old `0644` DB files);
  - never creates `~/.primer/` for an explicit path elsewhere;
  - a failed tighten is a `warn!`, not fatal.
- The GUI log sink and the config save create their directories privately.
- README notes the `0700` directory.

**Tests:**
- 6 new `paths` tests.
- `log_sink::open_log_file_creates_owner_only_directories`, confirmed red against the old `create_dir_all`.

**Gate:**
- `cargo test --workspace`: 1365/0.
- clippy `-D warnings` and fmt: clean.
- rustdoc for engine, gui and cli: clean.
- CI `cargo test (default features)`: green at close.

### #361 (`85ad79e` + docs `63736ea`): closes #340
All five gaps are pinned. Each new test was **mutation-checked**: the guarded regression was introduced by hand and the test went red.

1. **Env lookup:** `build_backend_params_with(config, env)` takes the env lookup as a closure; production passes `env_key`. Three tests use a recording closure:
   - the #206 cloud-fallback key is resolved;
   - `ANTHROPIC_API_KEY` is never read when cloud is unreachable;
   - the exact `OPENAI_COMPAT_API_KEY` name is read.

   The old test, which only bit when `ANTHROPIC_API_KEY` was set, was removed.
2. **KB locale on resume:** `stores::open_stores` composes the session store, learner and KB. A German learner resumed under an English cfg gives `knowledge.locale() == German`.
3. **Default session-DB path:** created under the injected home (`José` → `<home>/.primer/josé.db`).
4. **Real `Arc::clone` inheritance:** a main backend named "stub" under an `openai-compat` config appears in the extractor and comprehension ids.
5. **Model rebind:** the pure `rebind_main_model` replaces the inline qnn/llamacpp rebind.

**Docs:** the CLAUDE.md `primer-gui::wiring` paragraph is updated.

**Gate:**
- workspace tests: 1365/0.
- clippy: clean for `primer-gui` default, `speech` and `android-native`.
- rustdoc for gui: clean.

**Merge note:** #360 and #361 both touch `primer-gui/src/wiring/stores.rs`, in different hunks. Expect a clean auto-merge; if one conflicts, rebase it onto main after the other merges.

### Per-session sweep
- The oversized list is unchanged.
- The **inline-test detector is clean for the thirteenth session running**.
- `wiring/stage_tests.rs` is now 486 lines, close to the 500 guideline. Split it per stage if it grows.

## What's next (concrete acceptance criteria)

### 0. Owner review/merge: #360, #361
- **Acceptance:** CI is green and both merge.
- Then confirm the issues closed: `gh issue view 358 --json state; gh issue view 340 --json state`.

### 1. Sweep + inline-test detector: re-run EVERY session
Commands are in the resume block.

### 2. Remaining review-filed bugs (host-completable; TDD each: failing test, fix, `Closes #N` PR)
- **#351** (recommended next): subsystem ids still name the primary under `PrimaryUnavailable`, and override builds abort startup.
  - **Acceptance:** under `PrimaryUnavailable`, the classifier/extractor/comprehension identifiers name the fallback that serves them.
  - The new `subsystems_inherit_main_backend_via_arc_clone` test pattern (a uniquely named main backend shows up in the ids) is the template.
- **#352:** per-turn failover is only logged, and the GUI notice outlives a stopped voice session.
- **#331 / #332** (storage): cascade delete for `turn_classifications`; shape assertions for the v8 migration. Small and well-scoped.
- Older ones: #307–#316 (retrieval/storage/streaming), #325/#326 (voice).

### 3. Production-code splits (owner-approved lane, all feature-gated, so dual-verify)
- **`primer-inference/src/qnn/genie/real.rs` (566, `qnn`)** is the recommended one. Baselines:
  - `cargo test -p primer-inference`;
  - `cargo clippy -p primer-inference --features qnn --all-targets -- -D warnings`;
  - the rustdoc `qnn` arm.
- `primer-speech/src/macos/{tts.rs 668, stt.rs 504}` (`macos-native`).
- Low value: `state_machine/inner.rs` (506). Hardest: `primer-cli/src/main.rs` (~1350).

### Carried / owner-or-hardware-gated
- **#260**: Android-voice on-device acceptance.
- **#192**: manual macOS-native STT + injected TTS smoke.
- **#170 Stages B, E and F**: the OpenRAIL-M disclosure must ship before any default flip.
- **#166 item #1**: owner-run WhisperStream reuse smoke.
- **#135**: glib (blocked on Tauri 3).
- **Branch protection**: make `cargo test (default features)` a required check.
- QNN token stability, NPU tuning, latency-routing calibration.

## Open decisions / risks

- **#360 leaves existing `0644` DB files as they are.** The `0700` parent is what protects them.
  - If a user moves a DB out of `~/.primer/`, or passes an explicit `--session-db` in a shared directory, the file is still `0644`.
  - Tighten files on open only if the owner wants belt-and-braces.
- **#360's tighten runs only when a session opens** (CLI start, GUI session build). A GUI that only writes config or logs relies on creation-time `0700`, which is fine for fresh installs.
- **Windows and Android are not touched by #360.** Android's app-data dir is sandboxed; Windows relies on profile ACLs.
- **GUI log rotation runs only at startup** (marked `shortcut:` in `log_sink/mod.rs`).
- **The log file may hold child text.** Since #360 its directory is `0700` and the file `0600`, but a parent sharing it for support still shares that content.
- **Not seen live:**
  - no Dock launch of a bundled `.app` has been run;
  - the #353 banner hasn't been checked in `cargo tauri dev`.
- **Notice text is English-only** and operator-facing.
- **#359's Settings paths are hand-written strings.** A GUI label rename drifts silently.
- **Stale CMake cache after Xcode/SDK upgrades:**
  1. `find src/target -name CMakeCache.txt -maxdepth 6 | xargs grep -l 'MacOSX<old>'`.
  2. `cargo clean -p <those sys crates>`.
- **The `github` MCP server fails to connect** (bad Authorization header). The `gh` CLI works.
- **zsh gotchas:**
  - `$var` is not word-split; use `${=spec}`.
  - Quote `echo ===X`.
  - Use `git grep … -- '*.rs'`, not `--include` globs.

## Patterns to reuse, not reinvent

- **Testing env-var-dependent code:** inject the lookup as `impl Fn(&str) -> Option<String>`. Production passes the real reader. Tests pass a recording closure (`RefCell<Vec<String>>`) that answers from a slice and records every name asked for. Never `set_var`: it is unsafe and racy under the parallel runner. See `recording_env` in `primer-gui/src/wiring/stage_tests.rs`.
- **Proving `Arc` reuse without exposing internals:** hand in a backend whose `name()` differs from what a fresh build would produce. A `StubBackend` under an `openai-compat` config makes the ids read `llm:stub:<model>`.
- **Making a composition invariant testable:** move the two-step composition (open the store, then the KB under *its* locale) into a small stage fn (`open_stores`) that returns a named struct, and test the struct.
- **Mutation-check new tests:** back up the file to the scratchpad, apply a `sed`/`perl` mutation, run the filtered tests, restore. It takes about a minute per mutation and shows the test actually bites.
- **Owner-only dirs on Unix:** use `primer_engine::create_private_dir_all`, never bare `create_dir_all`, for anything under `~/.primer/`.
- **A Python `sub()` helper that asserts the match** stops before the write, but a following `&& git commit` in the same shell still runs if the python step is not chained with `&&`. Chain it, or check `git diff --stat` first. This session's CLAUDE.md edit missed the first commit this way.
- **Bug-fix recipe:**
  1. Branch off `origin/main`.
  2. Failing test, then fix, then green.
  3. Test + clippy each touched crate (plus its feature arms), then fmt.
  4. `git grep` the docs for drift.
  5. Commit with `Closes #N`, push, open the PR.
  6. File any out-of-scope half as an issue.
- **Messages shared by the CLI and GUI → name both forms in one string**, pinned with `assert_names_both` in `primer-engine/src/wiring/error_wording_tests.rs`.
- **A new UI notice = derive it in GUI wiring from data already in hand**, then forward it at every `SessionInfo {` site.
- **Feature-gated tests:** gate the whole module, then add a CI step.
- **Unreachable endpoint for network-failure tests:** `http://127.0.0.1:1`.
- **Stacked PRs:** retarget or rebase the child before merging it (the #344 loss).
- **Split recipe, substantive-line diff, Dependabot triage:** see `docs/handoffs/2026-10-16T202752Z.md`.
- Read `NEXT_SESSION.md` AFTER `git pull`, and check `git status` first.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status --short && git checkout main && git pull --ff-only && git log --oneline -6 && gh pr list --state open
# Open at close: #360, #361. After merge: gh issue view 358 --json state; gh issue view 340 --json state
S=/private/tmp/primer-scratch; mkdir -p "$S"   # or the session's scratchpad dir

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done

# === Next bug (#351): start here ===
gh issue view 351
git checkout -b fix/subsystem-ids-degraded-351 origin/main
~/.cargo/bin/cargo test -p primer-engine -p primer-gui 2>&1 | grep 'test result'   # baseline

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

- **#360:** `~/.primer/` is now owner-only (`0700`) for new and existing installs, so other logins on a shared computer can't read a child's transcripts. Closes #358.
- **#361:** the five primer-gui wiring behaviours from #340 are now pinned by tests, each mutation-checked. There are three small, behaviour-preserving seams (`build_backend_params_with`, `open_stores`, `rebind_main_model`). Closes #340.
- Workspace gate: 1365/0 on both branches.
