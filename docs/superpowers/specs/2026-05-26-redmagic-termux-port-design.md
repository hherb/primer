# RedMagic 11 — Phase 0 port via Termux (design)

**Date:** 2026-05-26
**Roadmap entry:** Phase 0 — "Runs on: …, RedMagic 11 Pro (via Termux or adb shell)"
**Status:** Spec — implementation plan to follow.

## Goal

Validate that the Phase 0 cloud-and-Ollama text REPL runs on RedMagic 11 Pro via Termux, document the path so a future contributor can reproduce it in under 30 minutes, probe whether hybrid retrieval (fastembed / ONNX runtime) works on Android ARM64, and lock in an Android cross-compile CI job so the build does not silently bit-rot.

The roadmap already lists RedMagic 11 Pro under Phase 0's "Runs on" line. Nothing in the codebase has ever been validated on the device. The user now has the phone, has Ollama installed and running 4B models with good performance, and wants to formally close the loop on Phase 0 portability before any Phase 1.1 (`LlamaCppBackend`) or Phase 1.2 (`QnnBackend`) work begins.

## Non-goals

- Voice (`--speech`) on Android. Phase 2 work; orthogonal to this slice. The whole `primer-speech` crate stays unbuilt in the default Android build.
- `LlamaCppBackend`. Phase 1.1, separate spec.
- `QnnBackend` for Snapdragon Hexagon NPU. Phase 1.2, separate spec.
- GUI (Tauri) on Android. Phase 3-adjacent; out.
- adb-shell + NDK build path as a user-facing flow. Termux is the documented runtime path; NDK cross-compile is a CI drift-guard only.

## Background — what the codebase already does right

Quick code-level probe (`grep -rn "target_os\|cfg(target" crates/`) confirms every `target_os = "macos"` cfg sits inside speech code that is `--features speech` only; the default Android build compiles zero macOS-specific source. `HOME`-based path resolution (`primer-engine::paths.rs`, `primer-cli::main.rs`) uses `$HOME` env var which Termux populates as `/data/data/com.termux/files/home`. The workspace pins Rust 1.88 in `rust-toolchain.toml`. Crates already in use are all platform-clean: `tokio`, `reqwest` (rustls feature), `rusqlite` (bundled SQLite), `clap`, `dotenvy`. Nothing in the default-features dependency graph phones home at build time.

The two genuine unknowns are (a) ORT 2.0.0-rc.10 prebuilt-binary availability for `aarch64-linux-android` (load-bearing for hybrid retrieval on the phone), and (b) whether any transitive dep pulls in a unix-only crate that fails to cross-compile.

## Architecture — what changes

This is a documentation and validation slice. No new crates, no new traits, no new backends. The trait-based architecture already insulates the engine from platform. The code surface is expected to be near-zero; the doc surface is the load-bearing deliverable.

### Shape of the work

The work is two PRs:

- **PR 1 (load-bearing 80%):** build + cloud REPL + on-device Ollama smoke + quickstart doc + CI Android cross-compile job.
- **PR 2 (uncertain 20%):** fastembed / ONNX runtime probe on Android ARM64, with documentation of one of three honest outcomes.

PR 1 is independently mergeable and gives the project the "validated on RedMagic" baseline. PR 2 ships separately because the fastembed outcome is genuinely unknown and is the wrong thing to gate a quickstart doc on.

### File-by-file changes (PR 1)

**New:**
- `docs/devel/redmagic-termux-quickstart.md` — the load-bearing deliverable. Sections:
  1. Prereqs: Termux from F-Droid (NOT Play Store; Play Store version is stale and broken for our use). Storage permission. `pkg update && pkg upgrade`.
  2. Toolchain install: `pkg install rust clang make pkg-config openssl-tool` then `rustup-init` to align with the 1.88 pin; document why (Termux's `pkg install rust` may lag).
  3. Clone + build: `git clone …; cd primer/src; cargo build --bin primer` (default features).
  4. Cloud run: `export ANTHROPIC_API_KEY=…` (or `~/.primer_env`), then `cargo run --bin primer -- --backend cloud --name X --age 8`.
  5. On-device Ollama run: `cargo run --bin primer -- --backend ollama --model <name> --ollama-url http://localhost:11434`.
  6. Session DB location on Android (`~/.primer/<slug>.db` → `/data/data/com.termux/files/home/.primer/<slug>.db`) — called out so users know where their child's data lives.
  7. Latency observations — informal, one short paragraph: time-to-first-token on cloud vs on-device 4B Ollama; subjective fluency; thermal note over 5 consecutive turns.
  8. Troubleshooting: any specific errors that surfaced during the user's actual run.

- `.github/workflows/ci.yml` — new job `android-aarch64-cross-compile`. Uses `dtolnay/rust-toolchain` with `aarch64-linux-android` target added, installs the Android NDK via `nttld/setup-ndk@v1`, sets `CC_aarch64_linux_android` / `CXX_aarch64_linux_android` / `AR_aarch64_linux_android` from the NDK, sets `CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER` to the NDK's `bin/aarch64-linux-android<API>-clang`. Runs `cargo build --target aarch64-linux-android --workspace --bin primer` (default features). Starts as `continue-on-error: true` so the workflow lands green even if the first run reveals a transitive-dep issue; flipped to required after a clean run on main.

**Modified:**
- `README.md` — add a line "Validated end-to-end on RedMagic 11 Pro via Termux (cloud + on-device Ollama)" once PR 1 lands green.
- `ROADMAP.md` — annotate the Phase 0 "Runs on" line: "✅ validated 2026-05-26 on RedMagic 11 Pro".
- `CLAUDE.md` — add a one-sentence pointer to the quickstart, plus a note: "Builds cleanly on Android ARM64 via Termux; `pkg install rust` may lag the 1.88 pin — use `rustup-init` if so."
- Per-crate `Cargo.toml`: changes only if surfaced by the cross-compile dry-run (e.g. a transitive dep needing `[target.'cfg(target_os = "android")'.dependencies]` gating). Expected: none.

### File-by-file changes (PR 2)

**Modified:**
- `docs/devel/redmagic-termux-quickstart.md` — appended section documenting the fastembed outcome on Termux. Three possible shapes:
  - **Works:** documented build command (`cargo build -p primer-cli --features embedding`), model-download note (BGE-M3 ~570 MB into `~/.cache/primer/models/`), latency observation, sample run.
  - **Builds but ORT runtime download fails:** documented manual workaround (manual `libonnxruntime.so` placement under the ORT cache dir).
  - **Doesn't build:** documented failure mode, link to a follow-up GitHub issue, note in CLAUDE.md that fastembed on Android is a known gap.
- `README.md` / `ROADMAP.md` — "validated on" annotations only if outcome is "works" or "works-with-caveats".
- `CLAUDE.md` — note Android-embedding gap if outcome is "doesn't build".

## Procedures

### Latency probe (PR 1)

- Single representative turn: ask "Why is the sky blue?" to a fresh session.
- Cloud (Sonnet 4.6): record wallclock first-token + total turn time. Run 3 times; report median + range.
- On-device Ollama (the installed 4B model): same. Report tok/s if `RUST_LOG=debug` exposes it; otherwise just total time.
- Thermal note: did the phone get warm under 5 consecutive turns? (The technical spec calls this out as a Phase 1 concern for sustained inference — informal observation only here.)
- One short paragraph in the doc, not a benchmark harness. The benchmark harness lives in Phase 1.1 alongside `LlamaCppBackend`.

### fastembed probe (PR 2)

1. On Termux: `cargo build -p primer-cli --features embedding` — record exact failure mode if any. ORT (`ort = 2.0.0-rc.10`) downloads a prebuilt binary from `cdn.pyke.io` at build time; if Android ARM64 isn't in the bundle, the build fails with a recognisable error.
2. If build succeeds: `cargo run --bin primer -- --backend ollama --model <X> --embedder-backend fastembed --name Test --age 8`. On first run, BGE-M3 model downloads (~570 MB) into `~/.cache/primer/models/`. Watch for OOM or hang.
3. If build fails: try ORT's feature flags to use a system-installed `libonnxruntime.so` (`load-dynamic` feature). Document. If still no path, open a follow-up issue tagged `android`, `embedding`, and note in CLAUDE.md.

Both outcomes (works / works-with-caveats / doesn't-build) are valid PR-2 endings. The deliverable is honest documentation, not necessarily a working binary.

## Known risks / uncertainties

1. **Termux's `pkg install rust` toolchain age.** Mitigation: docs steer users to `rustup-init` for the 1.88 pin. Low risk.
2. **ORT prebuilt binary for Android ARM64.** Genuinely unknown. The `cdn.pyke.io` bundle is Tier 1 for `x86_64-linux` / `aarch64-linux-musl` / macOS / Windows; Android Tier-2 inclusion has shifted between rc versions. This is the load-bearing fastembed unknown — and exactly why fastembed is in PR 2, not PR 1.
3. **rusqlite bundled-SQLite needs clang.** `pkg install clang make` is documented as a prereq; no expected runtime issue.
4. **GitHub Actions Android NDK setup.** Well-trodden — `nttld/setup-ndk@v1` and `android-actions/setup-android@v3` are both maintained. Low risk; might need a `CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER` env var pointing into the NDK's `bin/` (the action emits this).
5. **CI cross-compile lies about runtime behaviour.** A green cross-compile job tells us nothing about whether the binary works on the phone — it only catches link errors. The actual on-device validation is what the user does manually; CI is a drift-guard, not a correctness check. Documented as such in the workflow comment.
6. **`continue-on-error: true` on the CI job.** Lands green on day one; flipped to required after the first clean run on main. Same pattern as the prior macOS-DMG job's bring-up.
7. **A backend dep pulling in unix-only crate transitively.** Possible (e.g. `dbus` via something exotic, though nothing in the current dep tree hints at this). Mitigation: try `--target aarch64-linux-android` on the dev Mac before pushing; if a transitive dep fails to cross-compile, surface it as a `[target.'cfg(target_os = "android")'.dependencies]` exclusion or feature-gate.

## Success criteria

### PR 1

PR 1 lands green when, on the user's RedMagic in Termux:

- `cargo build --bin primer` succeeds (default features).
- `cargo test --workspace` passes (default features) — tests with hard `target_os = "linux"` / `"macos"` assumptions are documented or fixed.
- `cargo run --bin primer -- --backend cloud --name X --age 8` holds a 5-turn Socratic conversation against Anthropic.
- `cargo run --bin primer -- --backend ollama --model <X> --ollama-url http://localhost:11434` holds a 5-turn conversation against the installed 4B model.
- Session DB created under `~/.primer/<slug>.db`, persists across restarts, `--resume <uuid>` works.
- Quickstart doc renders cleanly, follows the verification-matrix style of NEXT_SESSION.md briefs (commands → expected → got).
- CI cross-compile job runs on every push (initially `continue-on-error: true`); flipped to required after a clean run on main.

### PR 2

PR 2 lands green when the fastembed outcome on Termux is documented unambiguously (one of the three shapes above) in the quickstart doc.

- If **works**: a turn with `--embedder-backend fastembed` against on-device Ollama succeeds without OOM, and hybrid retrieval visibly fires (e.g. visible passage selection differs from BM25-only).
- If **doesn't**: a follow-up issue is open with the exact error, a CLAUDE.md note records the Android-embedding gap, and CI cross-compile with `--features embedding` either lands green-or-`continue-on-error: true`.

## What this design deliberately leaves out

- **A "validated configurations" matrix.** YAGNI. There is one validated configuration (Termux + cloud + on-device 4B Ollama). Future configurations (Termux + `LlamaCppBackend`, NDK-built APK, etc.) are added as they are validated, not pre-allocated as table rows.
- **Benchmark numbers as a regression-tracked artifact.** The latency paragraph in the quickstart is a one-time observation. A benchmark harness with stored baselines lives in Phase 1.1 alongside `LlamaCppBackend`, where there's a real reason to track regressions across hardware.
- **A Termux-specific install script.** The quickstart is hand-followable. An automation script would add maintenance burden out of proportion to the saved keystrokes.
- **Pre-compiled APK distribution.** Not in scope for a Phase 0 validation. Phase 3 covers the device-as-product story.

## References

- [ROADMAP.md](../../../ROADMAP.md) — Phase 0 "Runs on" line; Phase 1.1 / 1.2 follow-ups.
- [docs/background_research/inference_architecture.md](../../background_research/inference_architecture.md) — Snapdragon 8 Elite latency targets, NPU plans.
- [docs/background_research/primer_technical_spec.md](../../background_research/primer_technical_spec.md) — "Phone-based parallel test" / "Phone-as-Primer viability" rationale.
- [CLAUDE.md](../../../CLAUDE.md) — workspace conventions, build-on-rustup discipline.
