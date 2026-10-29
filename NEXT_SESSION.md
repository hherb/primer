# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-10-28 (UTC).

**Three PRs are open against `main`** (no file overlap between them, so any merge order works):

| PR | Branch | What | Closes |
|---|---|---|---|
| **#374** | `fix/embedder-validation-optional-310` | Embedder-model validation propagates SELECT errors | #310 |
| **#375** | `fix/reembed-vec-count-311` | `reembed_kb` errors on a short embedder batch (+ README `--force` wording) | #311 |
| **#376** | `fix/fts-keep-keyword-terms-309` | FTS sanitizer keeps `AND`/`OR`/`NOT`/`NEAR` as terms (this brief rides on it) | #309 |

`main` is at `5dcbe5f` (#370).

**Since the last brief:**
- **#369 and #370 merged**; #366 and #331 are CLOSED.
- The #369 review produced two new issues: **#371** (`stop_voice_mode_inner`'s 5 s timeout detaches instead of aborting) and **#372** (consent-modal Cancel never persists `voice_mode_enabled=false`).
- **#352 stays open**, as planned.

## What we shipped this session

### #374 (`10d93ef`): closes #310
- **Problem:** all four `embedding_models` lookups (KB + session DB, validate + upsert) used `query_row(..).ok()`. A missing table, corrupt page or type error read as "unregistered", so the dim-mismatch hard error never fired.
- **Fix:** `.optional()` + `map_err` → only "no row" means unregistered; anything else propagates as `Knowledge` / `Storage`.
- **Tests:** per crate — corrupt-`dim` row and missing-table validate errors, upsert-on-corrupt-row errors, fresh-DB-unregistered Ok. Mutation-checked (reverting to `.ok()` turns both validate tests red). Knowledge's `embedding_model.rs` got its first test module.

### #375 (`cb57dc6` + `eac468e`): closes #311
- **Problem:** `reembed_kb` zipped inputs with vectors, so a short batch left tail passages unembedded and still returned `Ok`.
- **Fix:** `vecs.len() != chunk.len()` → `PrimerError::Knowledge`, checked **before** any upsert so a bad chunk writes nothing.
- **Tests:** a `DropsLastEmbedder` mock (StubEmbedder minus the last vector); red before the fix. Adds `async-trait` as a `primer-kb-load` dev-dep.
- **README:** the `reembed_kb` paragraph no longer claims `force` re-embeds everything. Filed the real gap as **#377**.

### #376 (`b37f8dc` + this brief's docs commit): closes #309
- **Problem:** both `sanitize_fts_phrase` copies dropped `AND`/`OR`/`NOT`/`NEAR`. Every token is already quoted, and quoted they are plain FTS5 terms, so the filter was pure recall loss.
- **Fix:** filter removed in `primer-knowledge/src/fts.rs` and `primer-storage/src/store/fts.rs`.
- **Tests:** knowledge end-to-end ("NEAR" retrieves the "near" passage; keyword-only hostile input raises no FTS5 error; red before the fix); storage unit + session-retrieval tests flipped from "dropped" to "kept as terms".
- **Retrieval check:** EN/DE BM25 retrieval-quality benchmarks green; ignored `bm25_floor_tripwire` unchanged (min 0.95, median 4.25, worst top-1 2.06). The `--features fastembed` hybrid benchmarks were **not** run.
- **Docs:** CLAUDE.md FTS bullet, `docs/devel/04` + `05`.

### Gate
- `cargo test --workspace` on the #375 branch: 1385/0.
- Per touched crate (each branch): test, clippy `--all-targets -D warnings`, fmt, rustdoc `--document-private-items` clean.
- **Not run this session:** the full rustdoc feature-arm loop and the `primer-gui` speech / android-native clippy arms — no touched crate is feature-gated.

> SHAs are branch-tip commits as pushed; squash-merge gives new ones on `main`.

### Per-session sweep
- **Inline-test detector:** clean for the seventeenth session running.
- **Oversized list:** unchanged.

## What's next (concrete acceptance criteria)

### 0. Owner review/merge: #374, #375, #376
- **Acceptance:** CI green, all three merge; then `gh issue view 309|310|311 --json state` are all CLOSED.

### 1. Sweep + inline-test detector: re-run EVERY session
Commands are in the resume block.

### 2. Bugs (host-completable; TDD each)
- **Smallest next picks:**
  - **#377** `reembed_kb --force` should re-embed everything. Acceptance: embed with a dim-8 stub, `force`-reembed with a dim-16 stub → every passage carries the dim-16 model's vector; decide (and test) what happens to the old `embedding_models` row.
  - **#372** consent-modal Cancel persists `voice_mode_enabled=false`. Acceptance: a contract test (`*_contract.rs` pattern) or command test proves Cancel writes the sticky toggle off, so the modal does not re-appear next launch.
  - **#308** kb-load seed loading not atomic. Acceptance: a malformed JSONL line leaves the KB with zero passages from that file (one transaction), test with a 3-line file whose line 2 is bad.
- **#371** (voice-loop abort on stop timeout) is bigger: needs a `JoinHandle::abort` design note first.
- **#352 remaining:** item 2 needs the owner decision below; items 1 and 4 need a design note before code.
- **Older:** #307, #312–#316, #325/#326, #362, #364, #365. Read each before choosing.

### 3. Production-code splits (owner-approved lane, all feature-gated, so dual-verify)
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
- **#376 changes BM25 query terms.** Common words "and"/"or"/"not" now take part in scoring. BM25 benchmarks and the floor tripwire are unchanged, but the **fastembed hybrid benchmarks were not run**. Run them before trusting the hybrid recall numbers: `~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test retrieval_quality_hybrid --test retrieval_quality_hybrid_de` (downloads ~570 MB BGE-M3 on first run).
- **#374 turns a previously-silent DB problem into a hard error** on hybrid retrieval. That is the point, but a user with a genuinely corrupt `embedding_models` row now sees a retrieval error instead of BM25-only results.
- **#375:** earlier chunks stay embedded when a later chunk fails. Safe, because reruns are idempotent, but the run reports an error, not a partial count.
- **#369 residuals (unchanged):** a session-build failure after the close still loses the text session (`shortcut:` in `start.rs`); not checked in a live `cargo tauri dev` session or on device.
- **#370:** v9 drop/rename runs with `foreign_keys = ON`; revisit if a future table ever FKs to `turn_classifications`.

### Carried from earlier sessions
- **Mislabelled rows from before #363:** rows in `turn_classifications` / `comprehension_classifiers` from earlier degraded sessions keep the wrong id. Under `PrimaryUnavailable`, a model-only override runs on the fallback kind; an invalid model there soft-fails per turn and is only logged (#352 item 4).
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
  - Use `git grep … -- '*.rs'`.

## Patterns to reuse, not reinvent

- **JS behaviour without a JS runner:** use an `include_str!` contract test in `primer-gui/src/*_contract.rs`. Slice the function body by a unique anchor string and assert the call is there. See `voice_stop_contract.rs` and `diagnostics_toggle_contract.rs`. Mutation-check by stashing the `ui/` changes and re-running.
- **Cross-file JS hooks:** app.js exports `window.primer*` functions at the bottom; voice.js lists the ones it uses in its header comment. Add new hooks in both places.
- **Migration tests:** pin *behaviour* (cascade fires, UNIQUE rejects), not DDL text. Build `fresh_vN_conn()` by chaining the earlier helpers. Fixtures should call the real `apply_vN_migrations`; never hand-roll the DDL.
- **Effective backend:** use `primer_engine::served_identity(...)` everywhere. Never re-derive it from `config.backend.kind`.
- **End-to-end degraded-primary GUI tests:** pair a `qnn` primary with a `cloud` fallback and an Inline key (`primary_unavailable_cloud_fallback_config()`).
- **Misbehaving-embedder tests:** wrap `StubEmbedder` and alter its output (`DropsLastEmbedder` in `primer-kb-load/src/tests.rs`). `Named` lives at `primer_core::speech::Named`.
- **Corrupt-row DB tests:** register a row, then `UPDATE … SET <int col> = 'not-a-number'` to force a read-side type error (`conn_with_corrupt_dim`).
- **Env-var-dependent code:** inject `impl Fn(&str) -> Option<String>`; never use `set_var`.
- **Mutation-check new tests:**
  1. Back up the file to the scratchpad.
  2. Apply the mutation with `perl -0pi`.
  3. Run the filtered tests.
  4. Restore the file.
- **Python `sub()` edits:** a helper that asserts the match count stops before the write.
- **Bug-fix recipe:**
  1. Branch off `origin/main`.
  2. Failing test, then fix, then green.
  3. Test + clippy each touched crate, plus its feature arms. Then fmt and rustdoc.
  4. `git grep` the docs for drift.
  5. Commit with `Closes #N`, push, open the PR.
  6. File any out-of-scope half as an issue.
- **Split recipe, substantive-line diff, Dependabot triage:** see `docs/handoffs/2026-10-16T202752Z.md`.
- **Driving a real `#[tauri::command]` in a test:** make it generic over `R: tauri::Runtime`, then call it with `tauri::test::mock_app()` + `app.manage(AppState::new(..))` + `app.state()`. See `commands/voice/tests.rs::app_with_text_session`.
- **Migration that changes an existing FK:** follow v9 (`pragma_foreign_key_list` guard + a create / copy / drop / rename tx).
- **Session start:** check `git status` first, and read `NEXT_SESSION.md` AFTER `git pull`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status --short && git checkout main && git pull --ff-only && git log --oneline -6 && gh pr list --state open
# Open at close: #374, #375, #376. After merge: for i in 309 310 311; do gh issue view $i --json state; done
S=/private/tmp/primer-scratch; mkdir -p "$S"   # or the session's scratchpad dir

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done

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

- **#374:** if the embedding registry in a database is damaged, the Primer now says so instead of silently searching with the wrong model. Closes #310.
- **#375:** re-embedding the knowledge base now stops with an error if the embedding server drops items, instead of quietly leaving some passages unsearchable. Closes #311.
- **#376:** questions containing "not", "near", "or" or "and" now use those words when searching ("why is the moon so near" keeps "near"). Closes #309.
