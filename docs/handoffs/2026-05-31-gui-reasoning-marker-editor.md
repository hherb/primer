# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-31 — Shipped the **GUI custom reasoning-marker editor** — the deferred half of the reasoning-token-stripping feature (PR #187, ROADMAP 0.3). Settings → Inference backend now has a "Reasoning markers" textarea (shown for `ollama`/`openai-compat`), so a GUI user can add custom `(open, close)` chain-of-thought marker pairs on top of the built-in defaults. Work is on branch **`feat/gui-reasoning-marker-editor`**, pushed, open as **PR #188** (https://github.com/hherb/primer/pull/188). Verified locally under the pinned **1.88** toolchain from `src/`: `fmt --check`, `clippy --workspace -D warnings`, `test --workspace` all green (0 failed / 4 pre-existing ignored; +15 new tests). **CI not yet confirmed at handoff time** — check `gh pr checks 188` before merge. **Manual GUI click-through not run** in the automated session (see Acceptance / risks).

## What we shipped this session — GUI reasoning-marker editor

**Branch:** `feat/gui-reasoning-marker-editor`. **PR:** #188 (open). Branched from `main` @ `8a8e0b9` (the #187 reasoning-stripping squash merge). 8 commits: 2 docs (spec+plan) + 6 implementation/docs. `main..HEAD` is exactly this feature.

Architecture: the textarea's raw text is stored verbatim as `BackendConfig.reasoning_markers: String`; a **pure, unit-tested Rust** `parse_reasoning_markers(&str) -> Vec<(String, String)>` converts it to pairs at session-wiring time; the ollama/openai-compat backends **append** them to the built-in default markers (never replace — child-safety invariant intact). The frontend is a verbatim pass-through (gather sends the textarea value as-is; populate echoes it back), so no parsing logic lives in untested JS. Empty textarea ⇒ empty Vec ⇒ defaults-only (no regression vs. before this branch).

Commit trail (oldest → newest):

| SHA | Commit |
| --- | --- |
| `3eb165b` | docs(spec): GUI reasoning-marker editor (deferred half of #187) |
| `f73f186` | docs(plan): GUI reasoning-marker editor implementation plan |
| `1824d8d` | feat(gui): pure `parse_reasoning_markers` textarea→pairs parser |
| `7d23fb3` | refactor(gui): annotate reasoning-marker guard; add dedup test |
| `f2ae2e5` | feat(gui): `reasoning_markers` String field on backend config + DTOs |
| `a5c211b` | feat(gui): wire `reasoning_markers` config into `BackendParams` |
| `1415195` | feat(gui): reasoning-markers textarea in Settings (populate/gather/reveal) |
| `79c15f5` | docs: GUI reasoning-marker editor shipped (README/ROADMAP/CLAUDE) |

### Components

1. **`primer-gui/src/reasoning_markers.rs`** (new) — pure `parse_reasoning_markers`. Rules: split into lines (CRLF-safe); per line trim, `split_once(char::is_whitespace)` → open = before first whitespace, close = remainder trimmed; drop a line with no whitespace (open-only) or empty close; blank lines ignored; close may contain internal spaces. 11 unit tests (incl. duplicate-pass-through regression guard). Mirrors the CLI's `pair_reasoning_markers` *semantics* but for line-based free text (different input shape, so not unified with the CLI — deliberate).
2. **`primer-gui/src/config.rs`** — `reasoning_markers: String` added to `BackendConfig` (default `""`, struct keeps `#[serde(default)]` so old `gui-config.json` loads fine), `BackendConfigView` (Serialize), `BackendConfigUpdate` (Deserialize, **NO** `#[serde(default)]` → mandatory IPC field). Threaded through `Default`, `From<&GuiConfig> for GuiConfigView`, and `GuiConfigUpdate::into_config`. 4 new round-trip tests; the **3 existing Update-DTO test JSONs** gained `"reasoning_markers": ""` (the mandatory-field break).
3. **`primer-gui/src/wiring.rs`** — `build_with_strategy` now calls `crate::reasoning_markers::parse_reasoning_markers(&backend_config.reasoning_markers)` instead of the old hard-coded `Vec::new()`.
4. **`primer-gui/ui/index.html`** — `<textarea id="f-backend-reasoning-markers">` in a `<label id="f-backend-reasoning-markers-field">` inside the Inference-backend `settings-grid`, with a format hint.
5. **`primer-gui/ui/settings.js`** — DOM refs; `populate()` reads `view.backend.reasoning_markers`; `gather()` sends `reasoning_markers: f.backendReasoningMarkers.value` verbatim (unconditional — mandatory even when the field is hidden); `applyBackendKindReveal()` shows the field only for ollama/openai-compat.

Spec: `docs/superpowers/specs/2026-05-31-gui-reasoning-marker-editor-design.md`
Plan: `docs/superpowers/plans/2026-05-31-gui-reasoning-marker-editor.md`

### Verification (this session, macOS host, pinned 1.88, from `src/`)
- `cargo +1.88 fmt --all -- --check` → clean.
- `cargo +1.88 clippy --workspace --all-targets -- -D warnings` → clean.
- `cargo +1.88 test --workspace --no-fail-fast` → all pass, 0 failed, 4 ignored (baseline 942 + 15 new = ~957 pass).
- Process: executed via subagent-driven development — fresh implementer per task + a two-stage (spec then quality) review per task + a final holistic cross-file review that walked the entire HTML→JS→IPC→config→wiring→backend data-flow chain and confirmed the child-safety invariant (custom markers append to, never replace, the built-in defaults).

**TOOLCHAIN GOTCHA (still the #1 trap):** always run cargo from inside `src/`. `rust-toolchain.toml` (pinning 1.88) lives at `src/rust-toolchain.toml` and is ONLY honored from there. From the repo root it silently resolves to user-default `stable`, whose newer lints fire on pre-existing untouched code and produce false "failures." Use `~/.cargo/bin/cargo +1.88 …` from `src/` when in doubt.

## What's next — by priority

**First: confirm CI green on PR #188, then merge.** `gh pr checks 188`; when green, `gh pr merge 188 --squash` (owner's call), then `git branch -D feat/gui-reasoning-marker-editor`.

**Owner manual check before/after merge (acceptance not run in the automated session):** build + run the GUI (`cd src && ~/.cargo/bin/cargo run --bin primer-gui`) and confirm:
1. Settings → Inference backend → select **ollama** (or openai-compat): the "Reasoning markers" textarea is **visible**; select **stub/cloud/qnn**: it is **hidden**.
2. Type `[[r]] [[/r]]`, Save & start a session; a model emitting `[[r]]…[[/r]]` around its reasoning has that span stripped from the child-visible reply.
3. Empty textarea ⇒ only built-in defaults strip.
4. Re-open Settings: the textarea shows `[[r]] [[/r]]` again (round-trip).

### Concrete actionable candidates (unchanged from last session minus the now-done GUI editor)

- **`openai_compat.rs` test-module split (low effort, tracked follow-up).** The file is ~755 lines, over the 500 guideline, almost entirely its `#[cfg(test)]` module. Split tests to a sibling file to bring production code under 500. Same applies as more backends grow reasoning wiring tests.
- **Step 1.2.0 — QAIRT install + chatapp_android device validation + run `qnn_bench`** (developer-side; standing highest-value QNN gate). Runbook `docs/devel/qnn-validation-runbook.md`. Acceptance: on the RedMagic 11 Pro via Termux, `cargo run --release --example qnn_bench --features qnn -- --bundle-dir ~/primer-bundles/qwen3-4b --duration-secs 900 --thermal-out ~/storage/shared/primer-thermal.csv` prints a verdict. Decode < 8 tok/s on Qwen3-4B → stop-and-reassess; pass (≥15 tok/s, <3s, ≤70 °C) → flip ROADMAP 1.2 ✅ with numbers + CSV.
- **#157 on-device Termux validation** (developer-side): does `ort-sys`'s build.rs fetch an `aarch64-linux-android` ONNX runtime from cdn.pyke.io? Until proven, Android default stays BM25-only (the `--no-default-features` android CI guard enforces it). This branch did NOT touch the embedder/Android path.
- **Branch protection on `main`** — still overdue; require the `cargo test (default features)` check.

### Open queue (issues — unchanged)

| #   | Title | State |
| --- | --- | --- |
| 170 | Stage B: wire Supertonic 3 as a voice-mode TTS backend (Hindi unlock) | Stage B merged (#175); A.5 spike next |
| 166 | Real-audio multi-utterance Whisper smoke (PR #164 follow-up) | needs human at mic + ggml-small.bin |
| 163 | split backends_common.rs once the test module grows | explicit deferral |
| 135 | bump glib → 0.20+ once Tauri 3 ships (RUSTSEC-2024-0429) | waits on Tauri 3 |
| 98  | split tests/common/sweep.rs into bm25/hybrid submodules | defer until 3rd locale |
| 41/40 | data/ingest disambiguation-regex scope / per-source attribution | self-deferred |
| 22  | primer-knowledge: cache prepared statements (Phase 0.2) | self-deferred |
| 21  | CLI: separate `--languages` preference list from bound `--language` | self-deferred |
| 20  | i18n placeholder validator false-fails on translator narrative | self-deferred |

## Open decisions / risks

- **Manual GUI click-through not run.** The end-to-end is proven at the unit level (parser + config round-trip + the existing `primer-inference` reasoning-strip tests) and via a hop-by-hop holistic review, but no human/automated click drove the actual Tauri window. Low risk (the data-flow chain was verified field-by-field), but the owner should do the 4-step check above.
- **Custom markers propagate to subsystem backends** (classifier/extractor/comprehension) — same as the CLI, because they share `BackendParams` via `build_backend`. Intentional and documented on the `BackendParams.reasoning_markers` field (stripping keeps subsystem JSON clean).
- **Permissive close rule.** `close` is "the rest of the line after the first whitespace", so a close marker may contain internal spaces (e.g. `<a> </a> tail` → `("<a>", "</a> tail")`). Natural for a line-based textarea; documented in the hint and spec. A user typing three tokens on a line gets a two-token close.
- **Gemma4 marker bytes still doc-sourced** (carried from #187): `("<|channel>","<channel|>")` from the ollama gemma4 docs. The `#[ignore]`'d `gemma4_live_reasoning_is_stripped` test in `primer-inference` confirms against a running ollama; cure on divergence is a one-line edit to `consts::reasoning::DEFAULT_MARKERS`.
- **QNN / Android risks carried forward** (ort-sys vendor liability, QNN ABI smoke unverified vs real libGenie.so, `qnn_bench` numbers device-unmeasured). See prior handoffs under `docs/handoffs/`.

## Patterns to reuse, not reinvent

New from this session:

- **Free-form GUI text → structured engine input: parse in pure Rust, keep the frontend a verbatim pass-through.** Store the raw textarea string in config, convert with a unit-tested `parse_*` function at the wiring boundary. Beats parsing in untested JS and round-trips the user's exact text. (`primer-gui::reasoning_markers` is the template.)
- **Adding a non-`serde(default)` field to a `*Update` DTO breaks every existing test JSON that deserializes it.** When you add a mandatory IPC field, grep for the DTO's literal test JSONs and add the key to each — and check none ends up with a duplicate key. (3 `BackendConfigUpdate` JSONs needed it this time.)
- **A new GUI backend field is a mechanical "thread it through 6 spots" change** that mirrors the adjacent siblings exactly: `Config` struct + `Default` + `View` + `From` + `Update` (no serde default) + `into_config`, plus the JS quartet (dom ref / populate / gather / reveal) and the HTML field. Copy the `qnn_bundle_dir` pattern verbatim.
- **A hidden form field must still be sent by `gather()` if its DTO field is mandatory.** Show/hide is cosmetic; the IPC payload is unconditional. Otherwise saving from a backend that hides the field fails to deserialize.

Carried forward (prior handoffs): stream-spanning text transforms need a stateful filter not per-chunk replace; put shared per-chunk logic in one helper both backends delegate to; `generate()` aggregates `generate_stream()` so fixing the stream fixes the non-streaming path; user-facing strings route through `render_inference_error` (single i18n boundary); model the question you're asking (bool not usize zero-check); feature-aware compile-time-conditional defaults; flipping a cargo default has CI blast radius; pure-core + thin-device-example for hardware-gated harnesses; not-a-secret config fields skip the View/Update Keep/Env dance; run cargo from `src/` so the 1.88 pin is honored.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer
git status                                      # clean; work pushed on feat/gui-reasoning-marker-editor
gh pr view 188                                  # the open PR for this session's work
gh pr checks 188                                # confirm CI before merge

# === Merge (after CI green + owner manual GUI check) ===
gh pr merge 188 --squash                        # owner's call
git branch -D feat/gui-reasoning-marker-editor  # after merge

# === Re-verify locally (ALWAYS from inside src/, with +1.88) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo +1.88 test --workspace --no-fail-fast        # 0 failed / 4 ignored

# === Targeted tests for this feature ===
~/.cargo/bin/cargo +1.88 test -p primer-gui reasoning_markers   # pure parser (11)
~/.cargo/bin/cargo +1.88 test -p primer-gui --lib config        # config round-trip (incl. 4 new)

# === Manual GUI acceptance (owner; needs a desktop session) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo run --bin primer-gui
# Settings → Inference backend → ollama: "Reasoning markers" textarea visible;
# stub/cloud/qnn: hidden. Type `[[r]] [[/r]]`, Save & start, confirm stripping; re-open to confirm round-trip.
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- **This session's caveat:** the feature is unit-verified on a macOS host under pinned 1.88 and passed a full cross-file holistic review, but the live Tauri click-through was NOT performed — the owner should run the 4-step manual check. CI on PR #188 was not yet confirmed green at handoff; check `gh pr checks 188`.
- The GUI is a full app — when a future brief calls it a scaffold, it's wrong; trust the code.
