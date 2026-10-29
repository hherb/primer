# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-10-29 09:05 (UTC).

**Two PRs are open against `main`** from this session:

| PR | Branch | What | Closes |
|---|---|---|---|
| **#384** | `fix/stream-eof-flush-313` | Stream buffers parse a final line/event left unterminated at EOF | #313 |
| **#385** | `fix/resume-drains-background-314` | `resume_session` drains the outgoing session's background tasks (this brief rides on it) | #314 |

- **Merge order:** independent. #384 touches only `primer-inference`; #385 touches `primer-pedagogy`. Both edit one bullet each in `CLAUDE.md`, on different lines, so there's no conflict.
- **`main` is at `9de7977`** (#382).

**Since the last brief:**
- **#379, #380, #381 and #382 all merged**; #308, #372 and #377 are CLOSED.

## What we shipped this session

### #384 (`7171946`): closes #313
- **Problem:** `SseBuffer`, `NdjsonBuffer` and `OpenAiSseBuffer` only yield `\n`-terminated units, and no spawn loop drained leftovers at EOF. A terminal payload flushed without a newline right before close was dropped:
  - **ollama / openai-compat:** the synthetic EOF chunk said `Stop`, masking a real `Length`.
  - **cloud:** the stream ended with no terminal chunk at all.
- **Fix:**
  - Each buffer has a `finish()` that terminates any residual line/event.
  - Each loop computes `let eof = match …`, drains as before, and on EOF drains once more before the existing synthetic-done / break.
  - The loop bodies are moved, not changed (`git diff -w` shows only the new lines).
- **Tests:** one per backend, driving the real `generate_stream` against a new `#[cfg(test)]` one-shot HTTP server, `primer-inference/src/test_http.rs` (`serve_once` + `collect`). All three were red before the fix. Mutation-checked.
- **Size:** `cloud.rs` is held at exactly 500 lines.
- **Docs:** CLAUDE.md streaming bullet, `docs/devel/03`.

### #385 (`e8edcac` + this brief's docs commit): closes #314
- **Problem:** `resume_session` didn't drain the task slots. The next turn then applied the old exchange's concepts onto the resumed session's turns.
- **Fix:** `await_pending_background()` *before* the session swap, then clear `last_extraction` / `last_comprehension`. Their field docs now name `new` + `resume_session`.
- **Test:** `resume_session_does_not_apply_previous_sessions_pending_extraction` (`lifecycle_tests.rs`). Red before the fix. Each half (drain, clear) mutation-checked separately.
- **Docs:** CLAUDE.md `dialogue_manager` bullet.

### Gate
- **Workspace:** `cargo test --workspace`: 1408/0 (both changes together, before splitting the branches).
- **#384:** `primer-inference` clippy `--all-targets -D warnings` + rustdoc `--document-private-items` on default / `qnn` / `llamacpp`.
- **#385:** `primer-pedagogy` clippy + rustdoc.
- **Both:** fmt clean.
- **Not run:**
  - the full rustdoc feature-arm loop (no other feature-gated crate was touched);
  - the fastembed hybrid benchmarks (still owed from #376).

> SHAs are branch-tip commits as pushed; squash-merge gives new ones on `main`.

### Per-session sweep
- **Inline-test detector:** clean for the nineteenth session running.
- **Oversized list:** unchanged. `cloud.rs` is at 500 lines, so it stays off the list.

## What's next (concrete acceptance criteria)

### 0. Owner review/merge: #384, #385
- **Acceptance:** CI green, both merged; then `gh issue view 313|314 --json state` are both CLOSED.

### 1. Sweep + inline-test detector: re-run EVERY session
Commands are in the resume block.

### 2. #307: a similarity floor for the hybrid vector leg (the lead pick; needs real embeddings)
- **What's wrong:**
  - `knn_scan` (`primer-knowledge/src/retrieval.rs`) has no cosine floor.
  - The DM passes `min_score = 0.0` on the hybrid path.
  - So hybrid **never returns empty**: "good morning" injects 5 irrelevant passages.
  - That also adds +0.45 to the router score, which tips small talk to the cloud under `--router-mode hybrid`.
- **Plan:**
  1. Add an off-topic query set (greetings, "can we stop now", emotional utterances; EN + DE) to `tests/common/`.
  2. Run it through `retrieval_quality_hybrid*` under `--features fastembed`, logging the top cosine per query for on-topic vs off-topic.
  3. Pick the floor from that gap and add it as a `primer_core::consts::retrieval` const (plus `HybridParams`).
  4. Apply it in `knn_scan`.
  5. Re-run the hybrid sweep.
- **Acceptance:**
  - 100% loose/strict recall preserved on the 91 EN queries and on the DE set (excluding `KNOWN_FAILING_QUERIES_DE_HYBRID`);
  - the off-topic set returns ≤1 passage each;
  - a structural `StubEmbedder` test pins "below the floor ⇒ dropped" on the default `cargo test`.
- **Cost:** ~570 MB BGE-M3 download on first fastembed run. Host-completable; no owner call needed unless the on/off-topic distributions overlap. If they do, report the numbers and ask.
- **Router follow-up:** decide whether `RoutingSignals.retrieved_passages` should count vector-only hits. That's a separate small issue if the floor alone doesn't fix the router inflation.

### 3. Other bugs (host-completable; TDD each)
- **#371** (voice-loop abort on stop timeout) needs a `JoinHandle::abort` design note first.
- **#352 remaining:** item 2 needs the owner decision below; items 1 and 4 need a design note before code.
- **Older:** #312, #315, #316, #325/#326, #362, #364, #365. Read each before choosing.

### 4. Production-code splits (owner-approved lane, all feature-gated, so dual-verify)
- **`primer-inference/src/qnn/genie/real.rs`** (566 lines, `qnn`) is the recommended one. Baselines: `cargo test -p primer-inference`; `cargo clippy -p primer-inference --features qnn --all-targets -- -D warnings`; the rustdoc `qnn` arm.
- **`primer-speech/src/macos/{tts.rs 668, stt.rs 504}`** (`macos-native`).
- **Low value:** `state_machine/inner.rs` (506). **Hardest:** `primer-cli/src/main.rs` (1353).

### Carried / owner-or-hardware-gated
- **#260:** Android-voice on-device acceptance; also re-check on device that a denied mic keeps the text session (#369).
- **#192:** manual macOS-native STT + injected TTS smoke.
- **#170 Stages B, E and F:** the OpenRAIL-M disclosure must ship before any default flip.
- **#166 item #1:** owner-run WhisperStream reuse smoke.
- **#135:** glib (blocked on Tauri 3).
- **Branch protection:** make `cargo test (default features)` a required check.
- **Other:** QNN token stability, NPU tuning, latency-routing calibration.

## Open decisions / risks

### #352 item 2: what counts as a pointless fallback? (owner decision, unchanged)
`--fallback-backend == --backend` with a different **model** is legitimate (small/big ollama models, or hybrid routing on one server).
- (a) notice only when kind **and** model match;
- (b) kind-only, worded as "no protection if the server goes down";
- (c) drop the item and keep the `tracing::warn!`.

**Recommendation:** (a), and downgrade or drop today's kind-only warn on the router path.

### This session
- **#385: a resume right after a turn can now block** for up to the background budget (default 10 s worst case) while the old tasks drain. Real hosts (CLI, GUI) resume on a fresh manager with nothing pending, so it's a no-op there. Aborting instead would lose the old exchange's learner updates.
- **#384: `finish()` only ever adds a terminator.** A truncated *partial* payload (connection cut mid-JSON) still fails to parse and is skipped with the existing `warn!`, the same as before. Only a complete-but-unterminated final unit is recovered.
- **#384:** the `serve_once` tests bind `127.0.0.1:0`. A CI sandbox that forbids loopback sockets would fail them (GitHub runners allow it; `primer-gui`'s download tests already do the same).
- **Still owed from #376:** the fastembed hybrid benchmarks (fold into the #307 work above):
  `~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test retrieval_quality_hybrid --test retrieval_quality_hybrid_de`.

### Carried from earlier sessions
- **#381:** a `force` reembed clears the locale's vectors up front. A part-way embedder failure degrades hybrid towards BM25 until a rerun. A model still used by another locale stays pinned at its old dim.
- **#382:** a *write* failure in a later seed file still leaves earlier files committed (`shortcut:` in `load_seed_files`). `bulk_insert_bench` not re-run after per-row transactions.
- **#374** turns a corrupt `embedding_models` row into a hard retrieval error, not BM25-only results. That is intended.
- **#369 residual:** a session-build failure after the close still loses the text session (`shortcut:` in `start.rs`).
- **#370:** v9 drop/rename runs with `foreign_keys = ON`; revisit if a future table ever FKs to `turn_classifications`.
- **Mislabelled rows from before #363:** rows in `turn_classifications` / `comprehension_classifiers` from earlier degraded sessions keep the wrong id (#352 item 4).
- **#360 permissions:**
  - Existing `0644` DB files are protected only by the `0700` parent directory.
  - The tighten step runs only when a session opens.
  - Windows and Android are untouched.
- **GUI logging:** log rotation runs only at startup (`shortcut:` in `log_sink/mod.rs`), and the log file may hold child text.
- **Notice text is English-only** and operator-facing. The Settings paths are hand-written strings.
- **Stale CMake cache after Xcode/SDK upgrades:**
  1. `find src/target -name CMakeCache.txt -maxdepth 6 | xargs grep -l 'MacOSX<old>'`
  2. `cargo clean -p <those sys crates>`
- **The `github` MCP server fails to connect** (bad Authorization header). The `gh` CLI works.
- **zsh gotchas:**
  - `$var` is not word-split; use `${=spec}`.
  - **Quote `echo ===X`**; an unquoted `===` errors with "== not found".
  - Use `git grep … -- '*.rs'` (zsh globs an unquoted `--include=*.rs` and errors "no matches found").
- **Cargo stops at the first failing test crate.** When mutation-checking across two crates (`-p a -p b`), re-run the second crate alone before concluding its test missed the mutant.

## Patterns to reuse, not reinvent

- **Driving a real streaming backend end to end (new):** `crate::test_http::serve_once(body)` in `primer-inference` answers one request with `200` + `body` and then closes, with no `Content-Length`. Point `OllamaBackend::new` / `OpenAiCompatBackend::new` / `CloudBackend::new` at the returned URL, then `crate::test_http::collect(stream)`. Use it for any spawn-loop behaviour (EOF, mid-stream error payloads) that buffer-level tests can't reach.
- **Restructuring a big match arm without retyping it (new):** a Python regex that captures the existing blocks and reassembles them, then `cargo fmt`, then verify with `git diff -w` that only the intended lines changed. Assert exactly one match before writing.
- **JS behaviour without a JS runner:** use an `include_str!` contract test in `primer-gui/src/*_contract.rs`. Slice the function body by a unique anchor string and assert the call is there. See `voice_stop_contract.rs` and `diagnostics_toggle_contract.rs`. Mutation-check by stashing the `ui/` changes and re-running.
- **Cross-file JS hooks:** app.js exports `window.primer*` functions at the bottom; voice.js lists the ones it uses in its header comment. Add new hooks in both places.
- **Migration tests:** pin *behaviour* (cascade fires, UNIQUE rejects), not DDL text. Build `fresh_vN_conn()` by chaining the earlier helpers. Fixtures should call the real `apply_vN_migrations`; never hand-roll the DDL.
- **Effective backend:** use `primer_engine::served_identity(...)` everywhere. Never re-derive it from `config.backend.kind`.
- **End-to-end degraded-primary GUI tests:** pair a `qnn` primary with a `cloud` fallback and an Inline key (`primary_unavailable_cloud_fallback_config()`).
- **Misbehaving-embedder tests:** wrap `StubEmbedder` and alter its output (`DropsLastEmbedder` in `primer-kb-load/src/tests.rs`). `Named` lives at `primer_core::speech::Named`.
- **Corrupt-row DB tests:** register a row, then `UPDATE … SET <int col> = 'not-a-number'` to force a read-side type error (`conn_with_corrupt_dim`).
- **Forcing a mid-transaction write failure:** open a second `Connection` on the same temp file and add `CREATE TRIGGER … BEFORE INSERT … WHEN NEW.id = 'bad' BEGIN SELECT RAISE(ABORT, 'x'); END`, or `DROP` the FTS table. See `insert_passages_rolls_back_*` in `primer-knowledge/src/tests.rs`.
- **Mutation-check a transaction:** remove the transaction entirely. Writing through `&conn` while a `BEGIN` is still open on the same connection is NOT a valid mutant: those writes are still inside the transaction.
- **DM background-task tests:** `await_pending_background()` is `pub(super)` and reachable from `dialogue_manager/tests/*`. Call it directly to force "what the next turn would apply" without scripting a second turn.
- **Env-var-dependent code:** inject `impl Fn(&str) -> Option<String>`; never use `set_var`. For seed discovery, test `load_seed_files(kb, &paths)` directly instead of setting `PRIMER_SEED_DIR`.
- **Mutation-check new tests:**
  1. Back up the file to the scratchpad.
  2. Apply the mutation with `perl -0pi`.
  3. Run the filtered tests.
  4. Restore the file.
- **Python `sub()` edits:** a helper that asserts the match count stops before the write.
- **Bug-fix recipe:**
  1. Branch off `origin/main`, and check `git branch --show-current` **before** editing. This session again started #314 on the previous fix's branch, and had to `git stash` → `checkout main` → `checkout -b` → `stash pop` before committing.
  2. Failing test, then fix, then green.
  3. Test + clippy each touched crate, plus its feature arms. Then fmt and rustdoc.
  4. `git grep` the docs for drift.
  5. Commit with `Closes #N`, push, open the PR.
  6. File any out-of-scope half as an issue.
- **Split recipe, substantive-line diff, Dependabot triage:** see `docs/handoffs/2026-10-16T202752Z.md`.
- **Driving a real `#[tauri::command]` in a test:** make it generic over `R: tauri::Runtime`, then call it with `tauri::test::mock_app()` + `app.manage(AppState::new(..))` + `app.state()`. See `commands/voice/tests.rs::app_with_text_session`. A command taking only `tauri::State` needs no generic.
- **Migration that changes an existing FK:** follow v9 (`pragma_foreign_key_list` guard + a create / copy / drop / rename tx).
- **Session start:** check `git status` first, and read `NEXT_SESSION.md` AFTER `git pull`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status --short && git checkout main && git pull --ff-only && git log --oneline -6 && gh pr list --state open
# Open at close: #384, #385. After merge: for i in 313 314; do gh issue view $i --json state; done
S=/private/tmp/primer-scratch; mkdir -p "$S"   # or the session's scratchpad dir

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done

# === #307 starting point (downloads ~570 MB BGE-M3 on first run) ===
gh issue view 307
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test retrieval_quality_hybrid --test retrieval_quality_hybrid_de -- --nocapture

# === Pick the next bug ===
gh issue list --state open --limit 40

# === Standard workspace gate (one cargo pass at a time) ===
~/.cargo/bin/cargo test --workspace 2>&1 | tee $S/ws.log | tail -3
grep -E 'test result: FAILED|^error' $S/ws.log  # expect empty
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo clippy -p primer-gui --features speech --all-targets -- -D warnings
~/.cargo/bin/cargo clippy -p primer-gui --features android-native --all-targets -- -D warnings
~/.cargo/bin/cargo fmt --all -- --check

# === Rustdoc gate (CI-enforced); feature arms mirror ci.yml ===
RUSTDOCFLAGS="-D warnings" ~/.cargo/bin/cargo doc --workspace --no-deps --document-private-items
for spec in "-p primer-speech --features voice-loop,silero,whisper,piper,cpal,supertonic" "-p primer-speech --features android-native" "-p primer-speech --features voice-loop,android-native" \
  "-p primer-inference --features qnn" "-p primer-inference --features llamacpp" "-p primer-gui --features speech" "-p primer-gui --features android-native" \
  "-p primer-cli --features speech" "-p primer-speech --features voice-loop,silero,cpal,macos-native" "-p primer-speech --features voice-loop,silero,cpal,macos-native-26"; do \
  RUSTDOCFLAGS="-D warnings" ~/.cargo/bin/cargo doc ${=spec} --no-deps --document-private-items >/dev/null 2>&1; echo "$spec exit=$?"; done
```

## Reporting back

- **#384:** when a local or cloud model server sends its very last message without a trailing newline and then hangs up, the Primer still notices that the reply was cut short. It can then recover instead of treating the truncated answer as complete. Closes #313.
- **#385:** resuming a different conversation on the same Primer instance can no longer mix the previous conversation's extracted concepts into it. Closes #314.
