# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-14 — **🎉 A FULL MULTI-TURN SOCRATIC CONVERSATION NOW RUNS ON THE HEXAGON NPU — near-instant, stable across turns, zero context overflow.** The owner's words: *"Performance was absolutely outstanding, near instant… feels like sitting on my MacBook."* The real 2K-context blocker turned out to be **not** prompt size but **Genie dialog-context accumulation** — fixed with a per-query `GenieDialog_reset`. The Primer's `QnnBackend` is now functionally complete on-device for real conversations. What's left is **answer-quality/pedagogy tuning** and a **responsive mobile GUI layout** — neither is a mystery.

**Context at session start:** clean `main` at `9e235ac` (PR #221 docs). **End state:** all work on branch `qnn-dialog-reset-and-prompt-budget`, opened as **PR #222**. Device (RedMagic 11 Pro, `912607710061`) runs the **reset-fix APK from committed source** (NOT a diagnostic build this time — smoke check ON and passing, WARN logging). cl2048 bundle staged, CmaFree ~631 MB.

## What we shipped this session (the big result)

The Primer's own `QnnBackend` runs a **3-turn conversation on the NPU with zero "Context limit exceeded"** in `genie.log` — validated on-device. Four changes, all TDD'd on host (228 qnn tests, full workspace green, 0 clippy):

1. **⭐ Per-query `GenieDialog_reset` (the actual blocker fix).** The Primer re-sends the *whole* prompt every query, and ONE Genie dialog handle is shared by the chat turn **and** the three background subsystems (classifier / extractor / comprehension — they `Arc::clone` the main backend, `kind: null` in `gui-config.json`). Genie *appends* every query to the same KV context, so it saturated the 2048-token window within a turn or two (the constant-`1938` "Context limit exceeded" we saw — `1938` was accumulated context, not a single prompt). Fix: bound `GenieDialog_reset` (QAIRT 2.45 `libGenie.so` exports it — confirmed via `llvm-nm`), added `reset()` to the `GenieDialog` trait (mock + real), and `generate_stream` calls `guard.reset()` before every `query_streaming`. Because `generate` routes through `generate_stream` (trait default), all four queries/turn reset. **Commit: `be25170`** (branch `qnn-dialog-reset-and-prompt-budget`, PR #222).
2. **Small-context prompt budget** (`primer-core::prompt_budget` — new pure module: `estimate_tokens`, `truncate_to_tokens` (sentence-boundary aware), `select_sections`). Wired into `build_turn_prompt` for `qnn:`-named backends only: 8-turn window (was 12), per-passage KB truncation to ~110 tokens, and a token-ceilinged system-prompt assembly that drops lowest-value optional sections first (vocab→retrieved→summary→knowledge) — **the Socratic base prompt is never trimmed**. Reduces per-query size (reply headroom); the reset is what fixed the saturation.
3. **#3 Chat-templated construction smoke check.** The smoke check passed a raw `"."` straight to Genie (no ChatML), so the model never emitted `<|im_end|>` and ran ~2045 tokens to context-full (~90 s) at startup. Now renders `"."` through the chat template (one user turn) so it stops promptly.
4. **#2 Graceful "context limit exceeded" (status 4) completion.** `classify_query_status` (pure, unit-tested) maps the context-limit code to a graceful turn-completion (the reply already streamed via the callback) instead of dropping the turn; all other non-success codes stay hard errors.

## What's next (concrete acceptance criteria)

### 1. ⭐ Responsive mobile GUI layout (the owner is actively hampered by this)
On the phone, the desktop debug/pedagogy sidebar + top status bar are **awkward in BOTH portrait and landscape** (portrait: sidebar takes the whole width, chat becomes a thin stripe, the "hide sidebar" control is off-screen with no horizontal-scroll affordance; landscape: also awkward per the owner). Pure GUI/CSS, independent of the now-working QNN backend.
- **Acceptance:** on phone widths in both orientations, the chat is usable full-width by default; the debug sidebar is collapsed/toggleable with an on-screen, reachable control; the status bar wraps/condenses instead of breaking. Test on the RedMagic in both orientations.

### 2. Pedagogy / answer-quality + rating tuning on the 4B NPU model
The owner: *"Quality of answers and ratings will have to be tuned."* The conversation works technically; now tune the Socratic behaviour and the classifier/comprehension ratings against the on-device 4B model.
- **Acceptance:** spot-check that the compressed prompt budget didn't dull Socratic behaviour (more questions than answers, comprehension-via-transfer); sanity-check `turn_classifications` / `turn_comprehensions` rows look reasonable on a real device session. Define specific tuning targets with the owner.

### 3. Real `qnn_bench` numbers (now unblocked — turns complete cleanly)
- **Acceptance:** run `cargo run --release --example qnn_bench --features qnn` on the device against the cl2048 bundle; record decode tok/s, TTFT, peak temp; compare to the targets (≥15 tok/s, <3 s TTFT, ≤70 °C). Then calibrate latency-aware routing (`--primary-ttft-budget-ms`).

### Carried / owner-or-hardware-gated
- The small-context budget consts (window 8, system budget 1100 EST, KB top-K 3, passage 110 tokens) are chars/4-estimate-based. With the reset fix + graceful handling, deep conversations complete even if a single prompt is large; tighten only if `genie.log` shows real overflow on long sessions. Calibrate against the `Context limit exceeded (P + G > C)` line.
- #170 Supertonic Stages E/F; #192 / #166 human-at-mic smokes; #157 Termux ONNX-runtime validation; #98 split `sweep.rs`; #135 glib bump on Tauri 3; #201 llamacpp BOS; llama.cpp device bench (owner-gated).

### Open queue (issues)

| #   | Title | State |
| --- | --- | --- |
| 201 | llamacpp BOS handling across model families | owner-gated (real-model smoke) |
| 192 | Manual smoke: macOS-native STT + injected TTS | needs human at mic |
| 170 | Supertonic 3 voice-mode TTS | Stage D shipped; E/F + manual gates next |
| 166 | Real-audio multi-utterance Whisper smoke | needs human at mic + ggml-small.bin |
| 135 | bump glib → 0.20+ once Tauri 3 ships | waits on Tauri 3 |
| 98  | split tests/common/sweep.rs | defer until 3rd locale |

## Open decisions / risks

- **`GenieDialog_reset` is now a REQUIRED symbol** in `primer-qnn-sys` (resolved at `GenieLibrary::open`, like `GenieDialog_free`). A `libGenie.so` that doesn't export it fails construction with `SymbolMissing` — acceptable: QAIRT 2.45 exports it (verified on-device via `llvm-nm`). If an older QAIRT is ever targeted, make it optional + skip the reset.
- **The reset is correct because the Primer is stateless-prompt** (re-sends system + history every query). Do NOT "optimize" by sending only the new turn and dropping the reset — the shared subsystem dialog would corrupt the chat KV. If a future design wants stateful KV reuse, the subsystems must get their own dialog first.
- **The device APK now matches committed source** (clean build, smoke check ON). Earlier handoffs warned about a diagnostic build; that's stale — this APK is the real thing.
- **Pedagogy on a 4B NPU model is the open quality question** — the owner explicitly flagged answer/rating tuning. Technically solid, pedagogically unverified at scale.
- **Branch protection ACTIVE on `main`** (required `cargo test (default features)`, strict). New work is PR-first.
- **The cl2048 bundle + V81 libs are git-ignored / off-repo.** Bundle staged on-device at `files/qnn-bundle` (`size: 2048`). Re-stage with `~/qnn-export-2048/stage-bundle.sh <SRC>` if needed.

## Patterns to reuse, not reinvent

New this session:
- **Genie dialogs accumulate KV across queries unless reset.** A persistent `GenieDialog` is stateful by design. A stateless-prompt engine (full prompt every query) that shares one dialog across chat + N subsystems MUST `GenieDialog_reset` before each query, or the context window saturates fast. The symptom is a *constant* "Context limit exceeded (X + gen > CTX)" where X is pinned near full and doesn't grow with prompt size — that's accumulated KV, not prompt size. (If X grew with the prompt, it'd be prompt size; constant X = accumulation.)
- **Reading behind a Genie status / confirming an exported symbol = `llvm-nm -D libGenie.so | grep <Sym>`** on the merged-native-libs `.so` in the Tauri build tree (`gen/android/app/build/intermediates/merged_native_libs/.../arm64-v8a/libGenie.so`). Far cheaper than a VERBOSE-log rebuild for "does this function exist."
- **VERBOSE Genie logging is a last resort** — it emits ~1.4M lines per reply and hangs the app on I/O. Prefer: read the WARN-level `Context limit exceeded` line for token counts; `llvm-nm` for symbols; pull `explorer.db` and `sqlite3` it for turn counts; a targeted app-internal debug-file write for prompt contents.
- **`pure prompt_budget` module pattern:** char/4 token proxy (`CHARS_PER_TOKEN`), sentence-boundary truncation, greedy `select_sections` — all in `primer-core::prompt_budget`, fully host-tested, consumed by the dialogue manager so the budget logic isn't inline in `build_turn_prompt`.

Carried (prior handoffs): Android scoped storage hides `adb`-pushed `/sdcard/Android/data/<pkg>` — stage app-internal via `adb push /data/local/tmp/<f>` → `run-as <pkg> cp`; **this RedMagic ROM has dead logcat + black screencap** — read app-internal files via `run-as <pkg> cat` (NOT `run-as <pkg> sh -c '<redirect>'`, which runs at cwd `/`; use `run-as <pkg> <cmd>` directly for `.primer/...` relative paths) + owner reads the screen; **a reboot leaves `/data/user/0/<pkg>` encrypted — owner enters PIN before `run-as`/app work**; **reboot maximizes CmaFree** (~631 MB right after boot); APK rebuild = `--no-default-features --features qnn`; cargo from `src/` with `+1.88`; commits touching `.github/workflows` need `gh auth refresh -s workflow`. Android host facts: JDK 21 = Android Studio JBR for `JAVA_HOME`; NDK at `/opt/homebrew/share/android-ndk`; throwaway build script `/tmp/build-qnn-apk.sh` wraps the env + `cargo-tauri android build --apk --debug --target aarch64 -- --no-default-features --features qnn`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status        # PR branch merged? then clean main

# === Host health check (default features = required gate) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 test --workspace
~/.cargo/bin/cargo +1.88 test -p primer-inference --features qnn qnn::      # incl. reset + budget tests

# === Device check (reset-fix APK installed, cl2048 staged) ===
ADB="$HOME/Library/Android/sdk/platform-tools/adb"
"$ADB" devices                                   # 912607710061; plug in + unlock (PIN) if empty
"$ADB" shell run-as org.theprimer.gui cat files/qnn-bundle/genie_config.json | grep -A1 '"size"'  # 2048
"$ADB" shell cat /proc/meminfo | grep -i cmafree

# === Read the on-device generation log (logcat is DEAD) ===
"$ADB" shell run-as org.theprimer.gui truncate -s 0 .primer/genie.log
"$ADB" shell monkey -p org.theprimer.gui -c android.intent.category.LAUNCHER 1
# owner sends messages, then:
"$ADB" shell run-as org.theprimer.gui cat .primer/genie.log | grep -i 'context limit'   # expect NONE now

# === Rebuild + reinstall the QNN APK after changes ===
bash /tmp/build-qnn-apk.sh   # wraps env + cargo-tauri android build --no-default-features --features qnn
"$ADB" install -r src/crates/primer-gui/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk

# === New code work: PR-first (branch protection on) ===
git checkout -b <branch> main && git push -u origin <branch> && gh pr create --base main ...
```

## Reporting back

- State plainly what works and what doesn't, by acceptance criterion.
- **This session's headline:** the per-query `GenieDialog_reset` fix made the Primer's `QnnBackend` run a **full multi-turn Socratic conversation on the Hexagon NPU — near-instant, stable, zero context overflow** (the real 2K blocker was dialog-KV accumulation across the chat + 3 shared-dialog subsystems, not prompt size). Shipped alongside: a small-context prompt budget, a chat-templated smoke check, and graceful context-limit completion. Remaining: a responsive mobile GUI layout, pedagogy/answer-quality tuning on the 4B NPU model, and real `qnn_bench` numbers (now unblocked).
- The GUI is a full app — when a future brief calls it a scaffold, it's wrong; trust the code.
