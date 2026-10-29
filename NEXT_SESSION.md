# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-10-29 21:57 (UTC).

**One PR is open against `main`** from this session:

| PR | Branch | What | Closes |
|---|---|---|---|
| **#390** | `fix/bm25-stopwords-386` | BM25 KB retrieval drops per-locale stopwords (this brief rides on it) | #386 |

- **`main` is at `fe86ee2`** (#387).

**Since the last brief:** #387 merged, #307 is CLOSED. #388 and #389 were filed from the #387 review (both are about the KB never being embedded by default).

## What we shipped this session

### #390 (`04e0645` + `38a25a1` + this brief's docs commit): closes #386
- **Measured first** (BM25-only, production `top_k=5` / `min_score=0.5`): 19/33 off-topic utterances retrieved, 81 passages in total. The per-token probe showed the very common words ("is", "the", "can") were already under the floor. The leaks came from mid-frequency function words ("we", "now", "your", "wir", "mehr", "keine", "ich") and content words ("good", "stop", "talk", "name", "Lust").
- **Fix:**
  - `primer_core::fts_util::sanitize_fts_phrase_excluding(query, exclude)`.
  - `primer_core::stopwords::is_stopword(locale, token)`. The lists are data files at `primer-core/stopwords/<pack_id>.txt` (en, de; hi empty = no-op), embedded with `include_str!`.
  - **Deviation from the brief:** the lists are NOT in the prompt pack, because `primer-knowledge` can't depend on `primer-pedagogy`. Adding a locale is still a data file plus one arm each in `list_source` and `set`.
  - `SqliteKnowledgeBase::bm25_search(.., drop_stopwords)` in `retrieval.rs`:
    - `retrieve` (BM25-only) drops stopwords;
    - the hybrid BM25 leg drops them **only when the vector leg is empty** (the default `:memory:` KB, #388); beside a non-empty vector leg it keeps them, because dropping them cost hybrid recall ("what is inside a tiny bug" lost `seed:en:insects`);
    - the vector leg now runs before the BM25 leg, so a gated query skips the BM25 work.
  - LTM retrieval is untouched.
  - Kept as terms: `not` / `nicht` / `near` (#309), and `how` (dropping it cost "how does my dog see in the dark" its eye passage).
- **Result:**
  - off-topic on the BM25-only path: 19/33 → **10/33**. The 10 match through content words and are pinned both ways in `tests/off_topic_bm25.rs` (`KNOWN_LEAKING_OFF_TOPIC_BM25`).
  - EN BM25 strict recall 21/24 → **22/24**: "why does the brain need oxygen from the lungs" now passes and has left `KNOWN_FAILING_QUERIES`. DE is unchanged.
  - hybrid fastembed EN + DE: still 100%. `KNOWN_LEAKING_OFF_TOPIC` (hybrid) is unchanged, because the vector leg alone fills those top 5s.
- **Router decision (from #386):** no change to `RoutingSignals.retrieved_passages`. The remaining hits are content-word matches the router can't tell from real relevance.
- **Tests:** stopword behaviour and list-hygiene tests, sanitizer predicate tests, three KB tests (BM25-only, locale-specific, plus the two hybrid directions). The KB tests are mutation-checked against the filter and both directions of the hybrid flag. `off_topic_bm25` is mutation-checked too: reverting the filter gives 9 drift lines.
- **Docs:** CLAUDE.md (FTS5 bullet, gate bullet, #45 paraphrase sentence, tripwire numbers, primer-knowledge layout); README (knowledge-base section). ROADMAP: no change (bug fix, no roadmap item).

### Gate
- `cargo test --workspace`: 1459/0.
- Workspace clippy `--all-targets -D warnings`; fastembed clippy for `primer-kb-load` + `primer-knowledge`; fmt; workspace rustdoc.
- fastembed: `off_topic_gate`, `retrieval_quality_hybrid{,_de}` green.
- BM25 sweeps (EN + DE) and `bm25_floor_tripwire` re-run.
- **Not run:**
  - the 54-cell hybrid sweeps (the hybrid leg with stored vectors is unchanged);
  - the rustdoc feature arms (no feature-gated code touched).

> SHAs are branch-tip commits as pushed; squash-merge gives new ones on `main`.

### Per-session sweep
- **Inline-test detector:** clean for the twenty-first session running.
- **Oversized list:** unchanged.

## What's next (concrete acceptance criteria)

### 0. Owner review/merge: #390
- **Acceptance:** CI green, merged; then `gh issue view 386 --json state` is CLOSED.

### 1. Sweep + inline-test detector: re-run EVERY session
Commands are in the resume block.

### 2. #388 + #389 together: the default KB is never embedded (recommended next; host-completable)
- **What's wrong:**
  - The default `:memory:` KB is seeded without vectors, so the default run's hybrid KB retrieval is BM25-only.
  - On that path `min_score = KB_MIN_SCORE = 0.0`, not the 0.5 BM25-only floor.
  - The #307 cosine gate never fires there.
  - #389: on a *partly* embedded KB the gate can drop on-topic BM25 hits.
- **Decide first (owner, or recommend):**
  - (a) embed after `auto_seed_if_empty` when an embedder is wired (BGE-M3 over ~91 passages each startup), and maybe default `--knowledge-db` to a persistent per-locale cache;
  - (b) only apply `KB_BM25_ONLY_MIN_SCORE` + a one-time `warn!` when the vector leg is empty.
  - Either way, #389 needs a coverage flag (`passages_missing_embedding()` exists).
- **Acceptance:**
  - a test that drives the *shipped* wiring (CLI/GUI store open + seed), not a hand `reembed_kb`;
  - the gate fires on a default run (if (a));
  - #389's partly embedded KB keeps BM25 hits for un-embedded passages;
  - recall benchmarks unchanged.
- **Note:** #390 already drops stopwords on the empty-vector-leg path, so small talk mostly stops leaking there today.

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

### #388 approach (new owner decision, see What's next §2)
**Recommendation:** (a) with a persistent per-locale KB cache under `~/.primer/`. Embedding ~91 passages on every `:memory:` startup is affordable on a desktop, but not on Android. Android stays `--embedder-backend none` anyway (#157).

### This session (#390)
- **The stopword lists are hand-built** (function words + contraction forms + fillers), validated only against the 33-utterance off-topic set and the 91 + 31 query benchmarks. Grow them with care: every added word must not appear in a real child question. Re-run `retrieval_quality{,_de}` + `off_topic_bm25` + `bm25_floor_tripwire`.
- **BM25 margin narrowed:** min top-K score 0.95 → 0.80 and worst top-1 2.06 → 1.12, against the 0.5 floor. The tripwire still passes (median 3.85 > 1.00), but there is less headroom for corpus growth.
- **Hindi has no list** (preview locale, no corpus); the filter is a no-op there.
- **The hybrid leg's stopword behaviour depends on the vector leg being empty.** On a *partly* embedded KB (#389) it keeps stopwords. That is consistent with the gate, but it means the un-embedded passages are matched with the noisier query.
- **The gate's debug line lost its `bm25_dropped` count** (the BM25 leg now runs after the gate).

### Carried from earlier sessions
- **#387 (off-topic gate):**
  - **Recall margin is thin:** 0.023 below the lowest benchmark query ("was ist schall", 0.473). A real child's on-topic question just under 0.45 now gets **no** grounding at all, not 5 weak passages. The 33-utterance off-topic set is small. Re-measure with `off_topic_gate_probe` when the corpus grows.
  - **Only `bge-m3` is gated.** ollama, openai-compat and the other fastembed variants keep the old never-empty behaviour until someone calibrates them (add an arm to `kb_query_gate_min_cosine`).
  - **The gate depends on the model id string.** Renaming `BGE_M3_MODEL_ID` silently ungates; the `query_gate_is_calibrated_for_bge_m3_only` test pins the lookup.
  - **`HybridParams` is shared with LTM** (`retrieve_session_turns_hybrid`), which ignores `min_top_cosine`. That is documented on the field.
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

- **Old-behaviour baseline for the stopword filter (new):** open a KB under `Locale::Hindi` (empty list) and `load_jsonl` the EN seed files into it. That gives the pre-#386 ranking side by side with the filtered one in a single probe run.
- **Before/after sweep numbers (new):** `git stash -u` → run the `#[ignore]`'d sweep → `git stash pop`. To see which `KNOWN_FAILING_*` entries now pass, empty the list temporarily (perl), run `retrieval_quality`, then restore from a scratchpad backup.
- **Per-token blame for a leaky query (new):** run each token alone through `kb.retrieve` at the production params. #386's probe showed the very common words ("is", "the", "can") were already under the 0.5 floor, and the leaks came from mid-frequency function words plus content words.
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
# Open at close: #390. After merge: gh issue view 386 --json state
S=/private/tmp/primer-scratch; mkdir -p "$S"   # or the session's scratchpad dir

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done

# === #388 / #389 starting point ===
gh issue view 388; gh issue view 389
git grep -n "auto_seed_if_empty" -- '*.rs' | grep -v tests
~/.cargo/bin/cargo test -p primer-kb-load --test off_topic_bm25 --test retrieval_quality --test retrieval_quality_de
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test off_topic_gate --test retrieval_quality_hybrid --test retrieval_quality_hybrid_de
~/.cargo/bin/cargo test -p primer-kb-load --test bm25_floor_tripwire -- --ignored --nocapture

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

- **#390:** when the Primer runs without the semantic search model (the Android setup, or whenever that model fails to load), "can we stop now" or "thank you" no longer pulls encyclopedia passages into its prompt. Common filler words are now ignored when matching the knowledge base, in English and German. That cut the small talk that retrieves anything from 19 of 33 phrases to 10. The 10 left use real words like "stop", "talk" or "name", which genuine questions use too. Real questions retrieve as well as before, and one English question that used to miss its passage now finds it. Closes #386.
