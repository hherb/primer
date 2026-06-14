# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-14 — **The responsive mobile GUI layout shipped.** On a phone the chat is now full-width, the debug/eval sidebar is a slide-in overlay drawer (backdrop/Esc dismiss), and the header condenses to icons so nothing clips in portrait or landscape. This clears the #1 "actively hampered" follow-up from the QNN-NPU session. Backend is solid (a full multi-turn Socratic conversation already runs near-instantly on the Hexagon NPU, PR #222). **What's left is owner-in-the-loop tuning + measurement, not a coding mystery:** pedagogy/answer-quality on the 4B NPU model, and real `qnn_bench` numbers.

**Context at session start:** clean `main` at `5f0e0ae` (PR #222 merged — per-query `GenieDialog_reset`). **End state:** responsive-GUI work on branch `responsive-mobile-gui`, opened as **PR #225** (CI running; merge when green). Device untouched this session (no rebuild needed — this is pure frontend).

## What we shipped this session

**Responsive mobile GUI layout** — commit **`3f66f79`** (branch `responsive-mobile-gui`, **PR #225**). Pure frontend (HTML/CSS/JS static assets) + one `cfg(test)` Rust contract module; no backend, no device rebuild.

Below a **940px** breakpoint:
1. **Chat goes full-width.** The old CSS grid handed the sidebar 280–340px while the chat shrank toward 0 (a thin stripe).
2. **Sidebar → right-edge slide-in overlay drawer.** Dim backdrop; tap-backdrop or **Esc** dismisses; default-closed on mobile. Slides over the chat, doesn't squeeze it.
3. **Header condenses to icons** (🎙 / 🗂 / ⚙ / ☰) so the toggle and all actions stay on-screen — previously the "Hide Sidebar" control ran off the right edge with no scroll affordance.

Design notes (so a future edit doesn't undo the load-bearing bits):
- **Mode-aware state.** Desktop uses `body.sidebar-collapsed` (default = open); mobile uses `body.sidebar-open` (default = closed). The toggle handler reads `window.matchMedia` to write the right class — both defaults fall out with no init-time juggling, and resizing across the breakpoint degrades gracefully.
- **Backdrop visibility is pure-CSS** (shown only under the breakpoint when the drawer is open) so a resize can never strand a dim layer over the desktop view. The drawer's box-shadow is **open-state-only** (a closed off-canvas drawer's left-pointing shadow would otherwise bleed across the right viewport edge).
- **No magic numbers.** The breakpoint is `MOBILE_BREAKPOINT_PX` in `app.js`, mirrored by the CSS `@media` query (CSS forbids `var()` in media conditions); drawer width / z-order / header height are named CSS custom properties in `:root`.
- **Desktop is byte-unchanged** (text buttons, two-column grid).

Tests: new `src/crates/primer-gui/src/responsive_layout_contract.rs` (11 tests) mirrors the existing `modal_dialog_contract` pattern — `include_str!` the static assets and assert their shape, since the GUI frontend has **no JS test runner** (no `package.json`). Pins the media query, the off-canvas transform, the backdrop element + rule, the icon/label spans, the matchMedia/`sidebar-open` toggle logic, the named breakpoint const, and **CSS↔JS breakpoint consistency**. Verified visually via a static render at **400px** (portrait), **900px** (RedMagic landscape), **1200px** (desktop unchanged). Full workspace suite green (50 test binaries, 0 failures); 0 clippy.

## What's next (concrete acceptance criteria)

### 1. ⭐ Pedagogy / answer-quality + rating tuning on the 4B NPU model (owner-in-the-loop)
The owner: *"Quality of answers and ratings will have to be tuned."* The conversation works technically; now tune the Socratic behaviour and the classifier/comprehension ratings against the on-device 4B model.
- **Acceptance:** spot-check that the compressed small-context prompt budget (8-turn window, KB top-K 3, per-passage 110-token truncation) didn't dull Socratic behaviour (more questions than answers, comprehension-via-transfer); sanity-check `turn_classifications` / `turn_comprehensions` rows look reasonable on a real device session. **Define specific tuning targets with the owner first** — this is a measurement/judgement task, not a blind code change.

### 2. Real `qnn_bench` numbers (unblocked — turns complete cleanly)
- **Acceptance:** run `cargo run --release --example qnn_bench --features qnn` on the device against the cl2048 bundle; record decode tok/s, TTFT, peak temp; compare to targets (≥15 tok/s, <3 s TTFT, ≤70 °C). Then calibrate latency-aware routing (`--primary-ttft-budget-ms`) from the measured TTFT.

### Carried / owner-or-hardware-gated
- The small-context budget consts (window 8, system budget ~1100 EST, KB top-K 3, passage 110 tokens) are chars/4-estimate-based. With the reset fix + graceful handling, deep conversations complete even if a single prompt is large; tighten only if `genie.log` shows real overflow on long sessions.
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

- **PR #225 is open, not merged.** Branch protection on `main` requires the `cargo test (default features)` check. Merge once CI is green (the change is static-asset + a `cfg(test)` module, so CI risk is low). The host suite passed locally.
- **The 940px breakpoint lives in TWO files** (`ui/styles.css` `@media` query + `ui/app.js` `MOBILE_BREAKPOINT_PX`) because CSS can't use a custom property in a media-query condition. `responsive_layout_contract::mobile_breakpoint_is_consistent_between_css_and_js` pins that they stay equal — if you change one, change both or the test fails loudly.
- **Don't "simplify" the two-class state model** (`sidebar-collapsed` desktop / `sidebar-open` mobile) into one — the dual classes are what make desktop-default-open AND mobile-default-closed both fall out with zero init-time JS. Collapsing them reintroduces a default-state bug at one breakpoint or the other.
- **The owner should still eyeball it on the real RedMagic** in both orientations — the static-render verification used a desktop browser at phone viewport sizes, which is faithful for CSS media queries but doesn't exercise the actual webview/touch. (No reason to expect surprises; the layout is standard.)
- **Pedagogy on a 4B NPU model is the open quality question** — technically solid, pedagogically unverified at scale. Owner explicitly flagged answer/rating tuning.
- **The cl2048 bundle + V81 libs are git-ignored / off-repo**, staged on-device at `files/qnn-bundle` (`size: 2048`). Re-stage with `~/qnn-export-2048/stage-bundle.sh <SRC>` if needed.

## Patterns to reuse, not reinvent

New this session:
- **Frontend "TDD" in this repo = a Rust `cfg(test)` contract module that `include_str!`s the UI assets and asserts their shape.** There is no JS test runner (no `package.json`). `responsive_layout_contract.rs` and `modal_dialog_contract.rs` are the templates: substring/parse checks on `ui/*.{html,css,js}` that make a frontend regression break `cargo test --workspace`. Strip JS comments before substring-asserting so a commented-out token can't false-pass.
- **Tighten a "feature present" CSS contract test to a feature-specific value, not a bare token.** `translateX` already existed in the `.toast` rule and false-passed; the test had to assert `translateX(100%)` (the drawer-specific off-canvas value) to actually test the drawer.
- **Static visual verification of responsive CSS:** `python3 -m http.server` the `ui/` dir, point Playwright at `http://localhost:<port>/index.html`, resize the viewport, and screenshot. Tauri `invoke` calls throw (no backend) but the static shell + CSS media queries render faithfully; add `sidebar-open` via `browser_evaluate` to see the open-drawer state. Clean up the server + the screenshot PNGs (they land in repo root) afterwards.

Carried (prior QNN/device handoffs, still true): Android scoped storage hides `adb`-pushed `/sdcard/Android/data/<pkg>` — stage app-internal via `adb push /data/local/tmp/<f>` → `run-as <pkg> cp`; this RedMagic ROM has **dead logcat + black screencap** — read app-internal files via `run-as <pkg> cat` + owner reads the screen; a reboot leaves `/data/user/0/<pkg>` encrypted until the owner enters the PIN; reboot maximizes CmaFree (~631 MB); QNN APK rebuild = `--no-default-features --features qnn`; cargo from `src/` with `+1.88`; commits touching `.github/workflows` need `gh auth refresh -s workflow`. Android host facts: JDK 21 = Android Studio JBR for `JAVA_HOME`; NDK at `/opt/homebrew/share/android-ndk`; throwaway build script `/tmp/build-qnn-apk.sh` wraps env + `cargo-tauri android build --apk --debug --target aarch64 -- --no-default-features --features qnn`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status        # PR #225 merged? then clean main

# === Host health check (default features = required gate) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 test --workspace
~/.cargo/bin/cargo +1.88 test -p primer-gui responsive_layout_contract   # the new 11 tests

# === Visually re-check the responsive layout (no device needed) ===
cd /Users/hherb/src/primer/src/crates/primer-gui/ui
python3 -m http.server 8973   # then open http://localhost:8973/index.html and resize the window
#   400px = portrait phone (icon header, full-width chat)
#   900px = RedMagic landscape (drawer mode)
#   1200px = desktop (two-column, text buttons — unchanged)

# === Device: pedagogy / qnn_bench (owner-in-the-loop) ===
ADB="$HOME/Library/Android/sdk/platform-tools/adb"
"$ADB" devices                                   # 912607710061; plug in + unlock (PIN) if empty
"$ADB" shell run-as org.theprimer.gui cat .primer/genie.log | grep -i 'context limit'   # expect NONE
# qnn_bench (device): cargo run --release --example qnn_bench --features qnn  (on-device build)

# === New code work: PR-first (branch protection on main) ===
git checkout main && git pull
git checkout -b <branch> main && git push -u origin <branch> && gh pr create --base main ...
```

## Reporting back

- State plainly what works and what doesn't, by acceptance criterion.
- **This session's headline:** the responsive mobile GUI layout shipped (PR #225, commit `3f66f79`) — below 940px the chat is full-width, the eval sidebar is a slide-in overlay drawer, and the header condenses to icons, so the phone is no longer awkward in portrait or landscape. Verified at 400/900/1200px; full host suite green, 0 clippy. Remaining: pedagogy/answer-quality tuning on the 4B NPU model and real `qnn_bench` numbers — both owner-in-the-loop.
- The GUI is a full app — when a future brief calls it a scaffold, it's wrong; trust the code.
