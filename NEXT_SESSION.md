# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-10-29 14:09 (UTC).

**One PR is open against `main`** from this session:

| PR | Branch | What | Closes |
|---|---|---|---|
| **#387** | `fix/hybrid-query-gate-307` | Off-topic query gate for hybrid KB retrieval (this brief rides on it) | #307 |

- **`main` is at `a12f1ea`** (#385).

**Since the last brief:** #384 and #385 both merged; #313 and #314 are CLOSED.

## What we shipped this session

### #387 (`1e80138` + this brief's docs commit): closes #307
- **Measured first:** best-passage BGE-M3 cosine on the shipped EN + DE corpora.
  - 122 on-topic benchmark queries: 0.473–0.772 (median 0.652).
  - 33 off-topic utterances (greetings, meta-talk, feelings): 0.365–0.473 (median 0.426).
  - **The ranges touch.** BM25 alone leaks 19/33 off-topic utterances through stopwords, so the per-passage `knn_scan` floor the issue suggested could not meet "≤1 passage each".
  - Reported to the owner, who chose **option A: a query-level gate at 0.45**.
- **Fix:**
  - `HybridParams.min_top_cosine: Option<f64>`. If the vector leg's best cosine is below it, `retrieve_hybrid_inner` returns nothing, BM25 hits included. An empty vector leg is never gated.
  - `primer_core::knowledge::kb_query_gate_min_cosine(model_id)` gives `Some(KB_QUERY_GATE_MIN_COSINE_BGE_M3 = 0.45)` for `bge-m3`, `None` for any other model; `HybridParams::default()` is `None`.
  - The DM's `kb_hybrid_params` sets it from `embedder.model_id()`. LTM is untouched.
  - `BGE_M3_MODEL_ID` now lives in `primer_core::consts::retrieval`; `primer_embedding` re-uses it.
- **Result at 0.45:** 0/122 on-topic lost; 26/33 off-topic now return nothing. The 7 leakers are pinned in `tests/common/off_topic.rs::KNOWN_LEAKING_OFF_TOPIC`.
- **Tests:**
  - 3 StubEmbedder tests in `primer-knowledge/src/tests.rs` (`retrieve_hybrid_query_gate_*`), mutation-checked.
  - `primer-kb-load/tests/off_topic_gate.rs` (fastembed): gated-vs-leaking pinned both ways, plus the `#[ignore]`'d `off_topic_gate_probe` for recalibration.
  - `retrieval_quality_hybrid{,_de}` now run with the gate on: 100% loose/strict preserved. The hybrid sweep harness runs gated too.
- **Docs:** CLAUDE.md hybrid-retrieval bullet.
- **Filed:** #386 (BM25 stopword matching; also covers the router-counting follow-up).

### Gate
- `cargo test --workspace`: 1426/0.
- Workspace clippy `--all-targets -D warnings`; `primer-kb-load` + `primer-embedding` fastembed clippy; fmt.
- Rustdoc: workspace, plus the `primer-embedding`/`primer-kb-load` fastembed arm.
- fastembed: `off_topic_gate`, `retrieval_quality_hybrid`, `retrieval_quality_hybrid_de` all green (this also discharges the benchmarks owed from #376).
- **Not run:**
  - the full 54-cell hybrid sweeps (EN + DE); only the production cell is verified, via the recall tests;
  - the other rustdoc feature arms (no feature-gated code in speech/inference/gui was touched).

> SHAs are branch-tip commits as pushed; squash-merge gives new ones on `main`.

### Per-session sweep
- **Inline-test detector:** clean for the twentieth session running.
- **Oversized list:** unchanged.

## What's next (concrete acceptance criteria)

### 0. Owner review/merge: #387
- **Acceptance:** CI green, merged; then `gh issue view 307 --json state` is CLOSED.

### 1. Sweep + inline-test detector: re-run EVERY session
Commands are in the resume block.

### 2. #386: BM25 stopword matching (the natural follow-up; host-completable)
- **What's wrong:** `sanitize_fts_phrase` OR-joins every token, so "can we stop now" gets 30 BM25 hits (top 4.01). This hits the BM25-only path (`--embedder-backend none`, Android, and the hybrid-error fallback) and the 7 gate leakers.
- **Plan:**
  1. Add per-locale stopword lists in the prompt pack, so a new locale stays data-only.
  2. Drop stopwords in the KB query sanitizer only. Keep #309's operator handling, and leave LTM alone unless measured.
  3. Measure with `tests/common/off_topic.rs` on the BM25-only path.
- **Acceptance:**
  - BM25-only 91-query EN + DE recall unchanged (`retrieval_quality{,_de}`; `KNOWN_FAILING_*` not grown);
  - hybrid fastembed recall still 100%;
  - off-topic BM25-only hits drop materially (report the before/after counts);
  - `KNOWN_LEAKING_OFF_TOPIC` shrinks, or stays with a note.
- **Risk:** child questions are stopword-heavy ("why is the sky blue"). If recall drops, report and ask.
- **Router:** decide in the same issue whether `RoutingSignals.retrieved_passages` needs changing once stopwords are gone.

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

### This session (#387)
- **Recall margin is thin:** 0.023 below the lowest benchmark query ("was ist schall", 0.473). A real child's on-topic question just under 0.45 now gets **no** grounding at all, not 5 weak passages. The 33-utterance off-topic set is small. Re-measure with `off_topic_gate_probe` when the corpus grows.
- **Only `bge-m3` is gated.** ollama, openai-compat and the other fastembed variants keep the old never-empty behaviour until someone calibrates them (add an arm to `kb_query_gate_min_cosine`).
- **The gate depends on the model id string.** Renaming `BGE_M3_MODEL_ID` silently ungates; the `query_gate_is_calibrated_for_bge_m3_only` test pins the lookup.
- **`HybridParams` is shared with LTM** (`retrieve_session_turns_hybrid`), which ignores `min_top_cosine`. That is documented on the field.

### Carried from earlier sessions
- **#385:** a resume right after a turn can block for up to the background budget (10 s worst case) while old tasks drain. It is a no-op on the real hosts.
- **#384:** `finish()` only recovers a complete-but-unterminated final unit. A truncated partial payload is still skipped with `warn!`. The `serve_once` tests bind loopback.
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

- **Calibrating a retrieval threshold (new):** measure the distribution *before* designing the fix. A throwaway `tests/zz_probe.rs` that embeds every seed passage (read the JSONL, skip `#` comment lines) and prints per-query best cosine + BM25 hit counts answered #307 in one run. The committed version is `off_topic_gate.rs::off_topic_gate_probe`. When the distributions overlap, report the numbers and let the owner pick the trade-off.
- **Struct-literal field additions across the workspace (new):** `git grep -n "HybridParams {"` to list them. A regex keyed on `min_score: …,\n source_filter` also hits `RetrievalParams` literals, so check each match.
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
# Open at close: #387. After merge: gh issue view 307 --json state
S=/private/tmp/primer-scratch; mkdir -p "$S"   # or the session's scratchpad dir

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done

# === #386 starting point (BM25-only path needs no model; fastembed re-check downloads ~570 MB BGE-M3 if not cached) ===
gh issue view 386
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality --test retrieval_quality_de
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test off_topic_gate --test retrieval_quality_hybrid --test retrieval_quality_hybrid_de
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test off_topic_gate -- --ignored off_topic_gate_probe --nocapture

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

- **#387:** when a child just says "good morning" or "can we stop now", the Primer no longer stuffs five unrelated encyclopedia passages into its prompt. That also stops simple small talk from being routed to the cloud model under hybrid routing. Real questions keep full retrieval, measured at 100% on the English and German benchmarks. Closes #307; the remaining stopword cases are filed as #386.
