# RedMagic 11 — Phase 0 Termux Port Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Validate the Phase 0 cloud-and-Ollama text REPL runs end-to-end on RedMagic 11 Pro via Termux, ship a reproducible quickstart doc, lock in an Android cross-compile CI drift-guard, and honestly document the fastembed/ONNX-runtime situation on Android ARM64.

**Architecture:** Two sequential PRs. PR 1 (load-bearing 80%) covers build + cloud + on-device Ollama + quickstart + CI cross-compile. PR 2 (uncertain 20%) covers the fastembed probe with three possible documented outcomes. This is mostly a documentation and CI slice; expected Rust code surface is near-zero because every `target_os = "macos"` cfg in the workspace already lives inside speech-feature-gated code that defaults off.

**Tech Stack:** Rust 1.88 (workspace pin in `src/rust-toolchain.toml`), Termux from F-Droid on RedMagic 11 Pro, GitHub Actions + `nttld/setup-ndk@v1` for cross-compile CI, on-device Ollama (already installed and running 4B models), Anthropic API for cloud backend.

**Two devices in play:**
- **Dev box (macOS):** code edits, commits, PR creation, CI workflow authoring, dev-box cross-compile dry-run.
- **Phone (RedMagic 11, Termux):** actual build + test + REPL runs; output captured to feed the quickstart doc.

Each task header says where it runs.

---

## PR 1 — Build, cloud + Ollama REPL, quickstart doc, CI cross-compile

### Task 1: Dev-box cross-compile dry-run (Mac)

Flush any transitive-dep platform issues before going to the phone. If something fails to cross-compile on the Mac, it will also fail on the phone — and Mac is faster to iterate.

**Files:**
- Read: existing `src/Cargo.toml`, per-crate `Cargo.toml` files (for context only)
- Modify (only if dry-run fails): the offending `Cargo.toml` — gate the unix-only dep behind `[target.'cfg(not(target_os = "android"))'.dependencies]`

- [ ] **Step 1: Install Android cross-compile target on dev box**

Run from anywhere:
```bash
rustup target add aarch64-linux-android
```
Expected: `info: component 'rust-std' for target 'aarch64-linux-android' is up to date` or `installed`.

- [ ] **Step 2: Install Android NDK on dev box (if not already)**

Run:
```bash
ls ~/Library/Android/sdk/ndk/ 2>/dev/null || brew install --cask android-ndk
```
Expected: a versioned NDK directory under `~/Library/Android/sdk/ndk/` or Homebrew installs one. Note the version and the absolute path to `bin/aarch64-linux-android<API>-clang` for the next step (API 24 is the lowest sensible floor; 33+ is fine).

- [ ] **Step 3: Cross-compile the binary**

Run from `/Users/hherb/src/primer/src`, substituting the actual NDK path and API level (replace `<NDK>` with e.g. `/Users/hherb/Library/Android/sdk/ndk/26.1.10909125` and `<API>` with `33`):
```bash
export CC_aarch64_linux_android=<NDK>/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android<API>-clang
export CXX_aarch64_linux_android=<NDK>/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android<API>-clang++
export AR_aarch64_linux_android=<NDK>/toolchains/llvm/prebuilt/darwin-x86_64/bin/llvm-ar
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$CC_aarch64_linux_android
cargo build --target aarch64-linux-android --bin primer
```
Expected: compiles to completion. `--bin primer` (without `--workspace`) builds only the `primer` binary's dep graph, which excludes `primer-gui` — Tauri 2.x deps don't cross-compile to Android (webkit2gtk etc.) and GUI on Android is explicitly out of Phase 0 scope per the spec.

If you forgot to add the target to the workspace-pinned 1.88 toolchain (the `rust-toolchain.toml`), the build fails with "can't find crate for core" — `rustup target add aarch64-linux-android --toolchain 1.88` fixes it. The default-toolchain target install (Step 1) is not sufficient. If any crate fails, capture the error and:
- If it's a unix-only transitive dep: gate via `[target.'cfg(not(target_os = "android"))'.dependencies]` in the offending crate's `Cargo.toml`. Re-run.
- If it's a C-toolchain issue (missing `libgcc`, wrong `--target` for clang): adjust the env vars; the prebuilt NDK toolchains have predictable names.
- If it's a `rusqlite` bundled-build issue: it shouldn't be — bundled SQLite needs only clang, which the NDK provides. If it does fail, capture the error verbatim for the quickstart troubleshooting section.

Note: this is a Mac dev-box validation, not a release artifact. The output binary won't be used; we're proving the workspace cross-compiles.

- [ ] **Step 4: If `Cargo.toml` was modified, commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/<changed>/Cargo.toml
git commit -m "$(cat <<'EOF'
build(android): gate <crate-name> behind cfg(not(target_os = "android"))

Surfaced by cross-compile dry-run on Mac (aarch64-linux-android).
<one-sentence reason — what the dep does on unix that's not needed on Android>.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```
If no changes, skip this step.

---

### Task 2: Phone-side prereqs (RedMagic, Termux)

Capture the actual commands that work on the user's device, including any package-name surprises, so the quickstart doc reflects reality.

**Files:** none yet — this task captures shell output for later doc-writing.

- [ ] **Step 1: Confirm Termux is from F-Droid, not Play Store**

In Termux on the phone, run:
```bash
pkg --version 2>/dev/null || apt --version
```
Expected: prints a version. If Termux is from Play Store, the package manager has been broken for years — uninstall and reinstall from F-Droid before continuing.

- [ ] **Step 2: Update Termux package index and base packages**

```bash
pkg update -y && pkg upgrade -y
```
Expected: completes without error. May take a few minutes the first time.

- [ ] **Step 3: Install build prereqs**

```bash
pkg install -y rust clang make pkg-config openssl-tool git
```
Note the version of `rustc` that ships:
```bash
rustc --version
```
Expected: prints e.g. `rustc 1.78.0` (stable line, may lag the 1.88 workspace pin).

- [ ] **Step 4: If pkg-installed rustc < 1.88, install rustup**

If the version printed by Step 3 is older than 1.88:
```bash
pkg install -y rustup
rustup default stable
rustup --version && rustc --version
```
Expected: rustc now ≥ 1.88.

If `pkg install rustup` doesn't work in your Termux repos, fall back to the manual rustup-init download (record the working command for the quickstart doc; do not silently invent one).

- [ ] **Step 5: Grant Termux storage access (for git clone target)**

```bash
termux-setup-storage
```
Tap "Allow" on the Android permission dialog. Expected: `~/storage/` symlinks appear.

- [ ] **Step 6: Capture every command + its output for the doc**

Note exact commands, the rustc version reported, and any errors/warnings. These feed Task 6 (writing the quickstart doc).

No commit yet — this task produces notes only.

---

### Task 3: Phone-side clone + build

**Files:** none yet — this task captures build output for the quickstart.

- [ ] **Step 1: Clone the repo**

```bash
cd ~
git clone https://github.com/<owner>/primer.git
cd primer/src
```
Expected: clone completes; `pwd` ends in `/primer/src` (the workspace root, not the repo root — workspace is under `src/`).

- [ ] **Step 2: Build with default features**

```bash
cargo build --bin primer
```
Expected: completes; output binary at `target/debug/primer`. First build will be slow (lots of crates compiled from source) — capture the approximate duration for the doc.

If build fails, capture the exact error. If the same error appeared in the Mac dry-run (Task 1) and was fixed there with a `cfg` gate, the fix should already be in `main` — pull the latest. If it's a new phone-only error, address it: most likely candidates are missing C deps (install via `pkg`), missing OpenSSL (we use rustls, so this is unlikely), or ORT-related (we shouldn't pull ORT on default features — if we do, that's a bug to fix).

- [ ] **Step 3: Run the stub backend to confirm the binary works**

```bash
cargo run --bin primer
```
Expected: greeting prompt appears; type something; Primer responds with a stub Socratic line; type `quit` to exit.

- [ ] **Step 4: Capture timings + any troubleshooting notes**

For the quickstart doc, note: clone duration, build duration (first), build duration (incremental — re-run `cargo build` after a no-op edit), debug binary size.

No commit yet.

---

### Task 4: Phone-side `cargo test --workspace`

This is the broadest portability check — many tests touch filesystem paths, env vars, FTS5, JSON parsing, etc. Most should pass; failures point at platform assumptions worth either fixing or documenting.

**Files:**
- Modify (only if a test has a hard `target_os = "linux"`/`"macos"` assumption that's wrong on Android): the offending test, gated with `#[cfg(not(target_os = "android"))]` or fixed in place.

- [ ] **Step 1: Run the workspace tests**

```bash
cargo test --workspace --no-fail-fast 2>&1 | tee /tmp/cargo-test-android.log
```
Expected: per the dev box's typical `858/0/3` count (passed/failed/ignored — check NEXT_SESSION.md verification matrix for the current number), the phone should be close. Failures fall into three buckets:
- **Genuine platform assumption in test code** (e.g. hardcoded `/tmp` path, expects `HOME` to be `/home/...`): fix the test or `#[cfg]` it.
- **Slow-on-phone-but-not-wrong** test that timed out: bump the timeout if there's a Tokio test timer, OR document in CLAUDE.md if the timeout is intentional.
- **Real bug under Android** that's worth fixing properly: rare; if it surfaces, capture and address.

- [ ] **Step 2: Categorize each failure**

For each failing test, paste the relevant log section into a scratch file on the phone (`~/primer-android-test-failures.txt`). Categorize per Step 1's buckets.

- [ ] **Step 3: Fix the trivially fixable ones**

For each test in the "genuine platform assumption" bucket:
- If the fix is small (`#[cfg(not(target_os = "android"))]` skip, or `env::temp_dir()` instead of hardcoded `/tmp`): edit the test file in place.
- If the fix is larger (a whole module assumes a unix layout): defer to a follow-up issue, add a one-sentence note in CLAUDE.md, and document in the quickstart's troubleshooting section.

- [ ] **Step 4: Re-run tests, confirm they pass**

```bash
cargo test --workspace --no-fail-fast 2>&1 | tee /tmp/cargo-test-android-after.log
```
Expected: pass count higher than Step 1; any remaining failures are documented.

- [ ] **Step 5: If any test file was modified, commit**

```bash
cd /Users/hherb/src/primer    # back on the dev box, or commit from phone if you prefer
git add src/crates/<modified-paths>
git commit -m "$(cat <<'EOF'
test(android): <one-line summary of the fix>

<2-3 sentences explaining the platform assumption that was wrong, and what
the new code does instead.>

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: Phone-side cloud + on-device Ollama REPL smoke

**Files:** none — captures REPL transcripts for the quickstart doc.

- [ ] **Step 1: Set up the Anthropic API key**

```bash
echo 'ANTHROPIC_API_KEY=sk-ant-...' >> ~/.primer_env
chmod 600 ~/.primer_env
```
(Use the actual key. `~/.primer_env` is the auto-loaded user-global env file per CLAUDE.md.)

- [ ] **Step 2: Cloud REPL — 5-turn Socratic conversation**

```bash
cd ~/primer/src
cargo run --bin primer -- --backend cloud --name TestChild --age 8
```
Have a 5-turn conversation about "why is the sky blue". After exit, capture:
- Wallclock time to first token (eyeball it; <2 s suggests good cellular)
- Whether the conversation felt natural
- Any errors or stalls

Repeat the same 5-turn conversation 3 times in fresh sessions. Note median + range of first-token time.

- [ ] **Step 3: Confirm session DB was created and persisted**

```bash
ls -la ~/.primer/
sqlite3 ~/.primer/testchild.db 'SELECT COUNT(*) FROM turns;'
```
Expected: `testchild.db` exists; turn count matches the conversation (≥ 10 = 5 child + 5 primer per conversation × 3 conversations = 30 turns, give or take).

- [ ] **Step 4: Confirm `--resume` works**

```bash
sqlite3 ~/.primer/testchild.db 'SELECT id FROM sessions ORDER BY started_at DESC LIMIT 1;'
# copy the uuid, then:
cargo run --bin primer -- --backend cloud --resume <uuid>
```
Expected: REPL opens with no greeting; you can continue the prior conversation. Type `quit`.

- [ ] **Step 5: On-device Ollama REPL — 5-turn conversation against your installed 4B model**

```bash
# Confirm Ollama is up and pick a model:
ollama list

# Then:
cargo run --bin primer -- --backend ollama --model <model-name> --name TestChild2 --age 8 --ollama-url http://localhost:11434
```
Have the same 5-turn "why is the sky blue" conversation. Capture:
- Wallclock first-token time (3 runs, median + range)
- tok/s if `RUST_LOG=debug cargo run …` exposes it; otherwise just total turn time
- Thermal observation after 5 consecutive turns (warm? hot? throttling visible as token slowdown?)
- Whether the dialogue felt comparable in quality to cloud — qualitative

- [ ] **Step 6: Capture all transcripts and timings**

Save to a scratch file on the phone (`~/primer-android-smoke-output.txt`). Feeds Task 6.

---

### Task 6: Write `docs/devel/redmagic-termux-quickstart.md` (dev box)

The load-bearing deliverable of PR 1. Reflects the actual commands and output captured in Tasks 2–5.

**Files:**
- Create: `/Users/hherb/src/primer/docs/devel/redmagic-termux-quickstart.md`

- [ ] **Step 1: Create the file with the structure below**

Use the captured output from Tasks 2–5 to fill the bracketed `<…>` slots. Do NOT leave any `<…>` placeholder in the committed file — every bracketed slot must be replaced with real captured content (or removed if not applicable).

```markdown
# RedMagic 11 — Phase 0 quickstart (Termux)

This guide gets the Primer's Phase 0 text REPL running on a RedMagic 11 Pro
(Snapdragon 8 Elite, 24 GB RAM) inside Termux, end-to-end, in well under
an hour. Validated 2026-05-26 on a stock device.

## Prereqs

- Termux installed **from F-Droid**, not Play Store. The Play Store
  version's package manager has been broken for years.
- Storage permission granted (Android settings → Apps → Termux → Permissions).
- An Anthropic API key for the cloud backend smoke.
- (Optional) Ollama already running on-device with a 4B model for the
  local-inference smoke. Installing Ollama in Termux is outside scope of
  this guide.

## Install build prereqs

```bash
pkg update -y && pkg upgrade -y
pkg install -y rust clang make pkg-config openssl-tool git
rustc --version    # expect ≥ 1.88
```

If pkg's `rustc` is older than 1.88 (the workspace pin in
`src/rust-toolchain.toml`), install rustup:

```bash
pkg install -y rustup
rustup default stable
rustc --version    # should now be ≥ 1.88
```

(If `pkg install rustup` doesn't work in your repo, fall back to the
manual rustup-init download per the rustup docs.)

Grant Termux storage access if you haven't:

```bash
termux-setup-storage
```

## Clone and build

```bash
cd ~
git clone https://github.com/<owner>/primer.git
cd primer/src   # NB: the workspace root is src/, not the repo root
cargo build --bin primer
```

First build: <captured duration from Task 3 Step 4>. Incremental builds:
<captured duration>. Debug binary size: <captured size>.

## Smoke: stub backend (no network)

```bash
cargo run --bin primer
```

Type something; the Primer should respond with a canned Socratic line.
Type `quit` to exit. This confirms the binary works end-to-end without
any external dependency.

## Smoke: cloud backend (Anthropic)

```bash
echo 'ANTHROPIC_API_KEY=sk-ant-...' >> ~/.primer_env
chmod 600 ~/.primer_env
cd ~/primer/src
cargo run --bin primer -- --backend cloud --name TestChild --age 8
```

Have a 5-turn conversation. Capture:

- Wallclock to first token: <median from Task 5 Step 2> (range
  <min>–<max> over 3 runs).
- Subjective fluency: <one short sentence>.

Session DB is created at `~/.primer/<slug>.db` (in Android terms:
`/data/data/com.termux/files/home/.primer/<slug>.db`). To resume:

```bash
sqlite3 ~/.primer/testchild.db \
    'SELECT id FROM sessions ORDER BY started_at DESC LIMIT 1;'
# copy the uuid, then:
cargo run --bin primer -- --backend cloud --resume <uuid>
```

## Smoke: on-device Ollama backend

```bash
ollama list   # confirm Ollama is up and pick a model
cargo run --bin primer -- \
    --backend ollama \
    --model <your-4B-model> \
    --name TestChild2 --age 8 \
    --ollama-url http://localhost:11434
```

Wallclock to first token: <captured median> (range <min>–<max>).
Subjective tok/s during streaming: <captured>. Thermal observation after
5 consecutive turns: <captured>. Conversation quality vs cloud:
<one short sentence>.

## Where your child's data lives

- Session DB: `~/.primer/<slug>.db` (per-learner).
- Long-term memory + summaries: same DB (schema v8).
- Knowledge base: in-memory by default (`:memory:`); pass
  `--knowledge-db <path>` to persist.

Per CLAUDE.md, all learner data stays local. Cloud inference is
stateless — only the per-turn messages travel.

## Troubleshooting

<Fill from any errors that came up during Tasks 2–5. If nothing came up,
include a "If you hit trouble, open an issue" line.>

## What works, what doesn't

✅ Cloud REPL with `--backend cloud`.
✅ On-device Ollama REPL with `--backend ollama`.
✅ Session persistence (`~/.primer/<slug>.db`) and `--resume`.
✅ Long-term memory + retrieval (auto-seeded corpus, FTS5).
✅ Vocabulary spaced repetition.
✅ Engagement classifier, concept extractor, comprehension classifier.

🟡 Hybrid retrieval (`--embedder-backend fastembed`) — pending PR 2 probe.

❌ Voice mode (`--speech`) — Android port deferred to Phase 2 work; not validated.
❌ GUI (Tauri desktop binary) — Phase 3-adjacent; not validated.
```

- [ ] **Step 2: Verify no `<…>` placeholders remain**

```bash
grep -n '<.*>' /Users/hherb/src/primer/docs/devel/redmagic-termux-quickstart.md | grep -v 'http' | grep -v '`<.*>`'
```
Expected: empty output (every `<…>` slot has been filled in OR appears inside a code fence as a literal command-line placeholder like `<uuid>` or `<model-name>`).

If output is non-empty, fill the remaining slots before committing.

- [ ] **Step 3: Commit the quickstart doc**

```bash
cd /Users/hherb/src/primer
git add docs/devel/redmagic-termux-quickstart.md
git commit -m "$(cat <<'EOF'
docs(android): RedMagic 11 Termux quickstart for Phase 0 text REPL

Validated end-to-end on a RedMagic 11 Pro: cloud REPL against Anthropic,
on-device REPL against installed Ollama 4B model, session persistence,
--resume. Latency and thermal observations captured inline. Closes the
long-standing roadmap claim that Phase 0 "runs on RedMagic 11 Pro".

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: Add Android cross-compile CI job (dev box)

**Files:**
- Modify: `/Users/hherb/src/primer/.github/workflows/ci.yml`

- [ ] **Step 1: Append the new job to `ci.yml`**

Open `.github/workflows/ci.yml`. After the existing `test:` job (and any other existing jobs), add a new top-level job. The exact content to add:

```yaml
  # Android cross-compile drift-guard. Catches transitive-dep platform
  # breakage before it surprises a developer in Termux. This job does
  # NOT validate runtime behaviour on Android — only that the workspace
  # cross-compiles. The actual on-device validation is documented in
  # docs/devel/redmagic-termux-quickstart.md. Starts as
  # continue-on-error so the workflow lands green; flip to required
  # after a clean run on main.
  android-cross-compile:
    name: cargo build (aarch64-linux-android)
    runs-on: ubuntu-latest
    continue-on-error: true
    defaults:
      run:
        working-directory: src
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust toolchain with Android target
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: aarch64-linux-android

      - name: Cache cargo registry + build
        uses: Swatinem/rust-cache@v2
        with:
          workspaces: src
          key: android-aarch64

      - name: Install Android NDK
        id: ndk
        uses: nttld/setup-ndk@v1
        with:
          ndk-version: r26d

      - name: Cross-compile primer binary
        env:
          # NDK toolchain paths. API 24 is the lowest sensible floor;
          # bumped if any transitive dep needs newer APIs.
          CC_aarch64_linux_android: ${{ steps.ndk.outputs.ndk-path }}/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android24-clang
          CXX_aarch64_linux_android: ${{ steps.ndk.outputs.ndk-path }}/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android24-clang++
          AR_aarch64_linux_android: ${{ steps.ndk.outputs.ndk-path }}/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-ar
          CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER: ${{ steps.ndk.outputs.ndk-path }}/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android24-clang
        # `--bin primer` only (no `--workspace`). primer-gui doesn't
        # cross-compile to Android and GUI on Android is out of Phase 0
        # scope per the spec.
        run: cargo build --target aarch64-linux-android --bin primer
```

- [ ] **Step 2: Verify the YAML parses**

```bash
cd /Users/hherb/src/primer
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))"
```
Expected: no output (success). If it errors, the indentation is off; fix and re-run.

- [ ] **Step 3: Commit the workflow change**

```bash
git add .github/workflows/ci.yml
git commit -m "$(cat <<'EOF'
ci(android): aarch64-linux-android cross-compile drift-guard

New job builds the workspace + primer binary for Android ARM64 on every
push. continue-on-error: true on landing so a first-run issue doesn't
block other CI; flip to required after a clean run on main. The job
catches link-time breakage from transitive-dep platform assumptions
but does not validate runtime behaviour — see
docs/devel/redmagic-termux-quickstart.md for the on-device test path.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: Update README + ROADMAP + CLAUDE.md (dev box)

**Files:**
- Modify: `/Users/hherb/src/primer/README.md`
- Modify: `/Users/hherb/src/primer/ROADMAP.md`
- Modify: `/Users/hherb/src/primer/CLAUDE.md`

- [ ] **Step 1: Update README.md "Runs on" line**

Open `README.md`, find the section that lists supported platforms (search for "Runs on" or "MacBook"). Append the line:

```markdown
- Validated end-to-end on RedMagic 11 Pro via Termux (cloud + on-device Ollama, 2026-05-26). See [docs/devel/redmagic-termux-quickstart.md](docs/devel/redmagic-termux-quickstart.md).
```

If the existing list is in a different style (table, paragraph), adapt the wording but keep the link.

- [ ] **Step 2: Update ROADMAP.md Phase 0 line**

Open `ROADMAP.md`. Find line 11:
```markdown
**Runs on:** MacBook, Spark DGX, any Linux box, RedMagic 11 Pro (via Termux or adb shell).
```

Replace with:
```markdown
**Runs on:** MacBook, Spark DGX, any Linux box, RedMagic 11 Pro (via Termux or adb shell). ✅ RedMagic 11 Pro validated 2026-05-26 — see [docs/devel/redmagic-termux-quickstart.md](docs/devel/redmagic-termux-quickstart.md).
```

- [ ] **Step 3: Update CLAUDE.md with a quickstart pointer + toolchain note**

Open `CLAUDE.md`. Find the "Build with rustup, not Homebrew rust" bullet (around the speech-section conventions). After that bullet, add a sibling bullet:

```markdown
- **Builds cleanly on Android ARM64 via Termux** — validated 2026-05-26 on RedMagic 11 Pro for the default-features Phase 0 text REPL (cloud + on-device Ollama). See [docs/devel/redmagic-termux-quickstart.md](docs/devel/redmagic-termux-quickstart.md). Termux's `pkg install rust` may lag the workspace's 1.88 pin — fall back to `pkg install rustup; rustup default stable` if so.
```

- [ ] **Step 4: Commit the doc updates**

```bash
git add README.md ROADMAP.md CLAUDE.md
git commit -m "$(cat <<'EOF'
docs: mark Phase 0 RedMagic 11 validation in README + ROADMAP + CLAUDE

Cross-references the new quickstart at
docs/devel/redmagic-termux-quickstart.md so a future contributor finds
the path from any entry point. Adds a CLAUDE.md note about the Termux
pkg-rust version lag.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 9: Push and open PR 1 (dev box)

- [ ] **Step 1: Push the branch**

```bash
cd /Users/hherb/src/primer
git push origin main   # or push to a feature branch if working off one
```
(If working off `main` directly per recent project rhythm, this pushes straight; if off a feature branch, `git push -u origin <branch>` and then `gh pr create`.)

- [ ] **Step 2: Open PR 1 if working off a feature branch**

```bash
gh pr create --title "Phase 0 RedMagic 11 Termux port — quickstart, validation, CI" --body "$(cat <<'EOF'
## Summary

- Validated end-to-end Phase 0 text REPL on RedMagic 11 Pro via Termux (cloud + on-device Ollama).
- Adds `docs/devel/redmagic-termux-quickstart.md` with full reproduction steps + latency + thermal observations.
- Adds GitHub Actions `aarch64-linux-android` cross-compile drift-guard (continue-on-error initially).
- Cross-references the validation in README, ROADMAP, CLAUDE.md.
- Spec at `docs/superpowers/specs/2026-05-26-redmagic-termux-port-design.md`.

## What this does NOT cover

Fastembed / hybrid retrieval on Android ARM64 is the subject of a separate follow-up PR — the ORT prebuilt-binary availability on Android is genuinely unknown and shouldn't gate this quickstart.

## Test plan

- [x] `cargo build --bin primer` on Termux (RedMagic 11 Pro).
- [x] `cargo test --workspace --no-fail-fast` on Termux — any failures documented or fixed.
- [x] 5-turn cloud REPL conversation (3 runs, latency captured).
- [x] 5-turn on-device Ollama REPL conversation (3 runs, latency + thermal captured).
- [x] Session DB created + `--resume` works.
- [ ] CI `android-cross-compile` job lands green on first push.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 3: After CI lands green, flip the cross-compile job to required**

After the first push lands and `android-cross-compile` is green, remove `continue-on-error: true` from `ci.yml` (Task 7 Step 1) and commit:

```bash
# Edit .github/workflows/ci.yml — delete the `continue-on-error: true` line.
git add .github/workflows/ci.yml
git commit -m "$(cat <<'EOF'
ci(android): require android-cross-compile to land green

First-run validation per the comment block landed clean on main. Flipping
from continue-on-error to required so future regressions block merges.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
git push
```

PR 1 is complete.

---

## PR 2 — fastembed / ONNX runtime probe on Android

The deliverable is honest documentation of one of three outcomes. PR 2 ships when the outcome is documented in the quickstart, even if the outcome is "doesn't build".

### Task 10: Probe `cargo build --features embedding` on Termux (phone)

**Files:** none yet — captures outcome.

- [ ] **Step 1: Attempt the embedding-feature build**

```bash
cd ~/primer/src
cargo build -p primer-cli --features embedding 2>&1 | tee /tmp/embedding-build.log
```

Wait for the build to either complete or fail. The most likely failure point is ORT 2.0.0-rc.10's `build.rs` trying to download a prebuilt `libonnxruntime.so` from `cdn.pyke.io` — if Android ARM64 isn't in the bundle for that rc version, you'll see an error referencing the download URL or a "no prebuilt binary for target" message.

- [ ] **Step 2: Classify the outcome**

Three outcome shapes per the spec:

- **Outcome A — Works:** build completes. Skip to Task 11.
- **Outcome B — Builds but ORT runtime download failed:** the build itself completed (maybe via a fallback path), but at runtime, attempting `--embedder-backend fastembed` fails with a `dlopen`-style error or "ONNX Runtime not found". Skip to Task 12.
- **Outcome C — Doesn't build:** `cargo build` returns non-zero with a clear ORT-related error. Skip to Task 13.

Save the exact error/output to `~/primer-android-embedding-outcome.txt`.

---

### Task 11: Outcome A — fastembed works (only run if Task 10 → A)

- [ ] **Step 1: Run a hybrid-retrieval turn**

```bash
cargo run --bin primer -- \
    --backend ollama --model <4B-model> --ollama-url http://localhost:11434 \
    --embedder-backend fastembed \
    --name TestChild3 --age 8
```

On first run the BGE-M3 model downloads (~570 MB) into `~/.cache/primer/models/`. Watch for OOM or hang during download or first turn. Have a 3-turn conversation; type `quit`.

- [ ] **Step 2: Verify hybrid retrieval visibly fires**

Re-run the same conversation question in a fresh session with `--embedder-backend none` (the BM25-only path) and again with `--embedder-backend fastembed`. Either by reading `RUST_LOG=debug` output, or by running the same first child-turn in both modes and observing the retrieved passages section of the system prompt, confirm that the two retrieval paths surface different passage orderings.

- [ ] **Step 3: Append "Outcome A" section to the quickstart**

Append to `/Users/hherb/src/primer/docs/devel/redmagic-termux-quickstart.md`:

```markdown
## Hybrid retrieval (`--embedder-backend fastembed`)

✅ Works on RedMagic 11 Pro as of 2026-05-26.

```bash
cargo build -p primer-cli --features embedding
cargo run --bin primer -- \
    --backend ollama --model <your-4B-model> \
    --ollama-url http://localhost:11434 \
    --embedder-backend fastembed \
    --name TestChild --age 8
```

First run downloads BGE-M3 (~570 MB) into `~/.cache/primer/models/`.
Subsequent runs reuse the cached model.

Latency observations on top of the cloud baseline above:

- Per-turn overhead from embedding the child's input: <captured ms>.
- First-run model download: <captured duration>.
- Memory headroom on 24 GB device under ollama + embedding: <captured>.
```

Replace `<captured …>` with real values.

- [ ] **Step 4: Update README + ROADMAP + CLAUDE.md**

Edit `README.md`, `ROADMAP.md`, and `CLAUDE.md` to annotate hybrid retrieval as validated on RedMagic 11 Pro. One line each, paralleling Task 8 Step 1–3.

- [ ] **Step 5: Optionally extend CI to verify the embedding feature builds**

In `.github/workflows/ci.yml`, add a second cross-compile job (or extend the existing one) to run:

```yaml
        run: cargo build --target aarch64-linux-android --bin primer --features primer-cli/embedding
```

Land it `continue-on-error: true` first.

- [ ] **Step 6: Commit and open PR 2**

```bash
cd /Users/hherb/src/primer
git add docs/devel/redmagic-termux-quickstart.md README.md ROADMAP.md CLAUDE.md .github/workflows/ci.yml
git commit -m "$(cat <<'EOF'
docs(android): hybrid retrieval (--embedder-backend fastembed) validated on RedMagic

ORT 2.0.0-rc.10's prebuilt binary for aarch64-linux-android worked on
Termux out of the box; BGE-M3 downloads to ~/.cache/primer/models/ on
first run. Per-turn overhead and memory headroom observations captured
inline. Validated 2026-05-26 on a 24 GB RedMagic 11 Pro.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
gh pr create --title "RedMagic 11: hybrid retrieval validated" --body "Outcome A of the Phase 0 fastembed probe — works. See docs/devel/redmagic-termux-quickstart.md for the new section."
```

Skip Tasks 12 and 13.

---

### Task 12: Outcome B — Builds but runtime download fails (only run if Task 10 → B)

- [ ] **Step 1: Capture the exact runtime error**

Run the fastembed turn (Task 11 Step 1 command). Paste the exact error to `~/primer-android-embedding-outcome.txt`.

- [ ] **Step 2: Investigate the ORT system-library workaround**

ORT exposes a `load-dynamic` feature for using a system-installed `libonnxruntime.so` instead of the cdn.pyke.io prebuilt. Check whether Termux has ONNX Runtime available:

```bash
pkg search onnx 2>/dev/null
```

If yes, install it; set `ORT_DYLIB_PATH` to the resulting `.so`; retry the fastembed turn. If no, document the absence and the path that would be needed.

- [ ] **Step 3: Append "Outcome B" section to the quickstart**

Append to `docs/devel/redmagic-termux-quickstart.md`:

```markdown
## Hybrid retrieval (`--embedder-backend fastembed`)

🟡 Partial — builds but the prebuilt ORT runtime doesn't resolve at runtime.

```
<exact error captured in Task 12 Step 1>
```

Workaround attempted: <result of Task 12 Step 2>. If a working workaround
exists, the steps are:

<numbered steps if applicable>

If no workaround works on the user's specific Termux build, fall back to
`--embedder-backend none` (BM25-only retrieval). The retrieval-quality
benchmark suite is tuned to pass at 100% strict recall on the 91-query
benchmark under BM25-only defaults, so this is a graceful fallback, not
a broken feature.

Follow-up issue: <link to opened GitHub issue>.
```

- [ ] **Step 4: Open the follow-up issue**

```bash
gh issue create \
    --title "fastembed/ORT prebuilt binary fails to resolve at runtime on Android ARM64 (Termux)" \
    --label "android,embedding" \
    --body "$(cat <<'EOF'
ORT 2.0.0-rc.10's prebuilt binary downloaded from cdn.pyke.io builds cleanly on Termux (RedMagic 11 Pro, 2026-05-26) but fails to load at runtime with:

```
<exact error>
```

Workaround attempted: <Task 12 Step 2 result>.

See `docs/devel/redmagic-termux-quickstart.md` for the documented partial state. Until this is resolved, `--embedder-backend none` (BM25-only) is the recommended Android setting; retrieval-quality benchmarks pass at 100% strict recall under BM25-only defaults on the 91-query corpus.
EOF
)"
```

- [ ] **Step 5: Update CLAUDE.md with the gap**

Add to CLAUDE.md (under the embedding-related conventions):

```markdown
- **Hybrid retrieval (`--embedder-backend fastembed`) is BM25-fallback only on Android ARM64** as of 2026-05-26 — ORT prebuilt binary builds but fails to load at runtime in Termux on RedMagic 11. See issue #<N>. BM25-only is the recommended Android setting; the retrieval-quality benchmark suite passes at 100% strict recall under BM25-only defaults so this is not a regression.
```

- [ ] **Step 6: Commit and open PR 2**

```bash
cd /Users/hherb/src/primer
git add docs/devel/redmagic-termux-quickstart.md CLAUDE.md
git commit -m "$(cat <<'EOF'
docs(android): hybrid retrieval — builds but ORT runtime broken on Termux

ORT 2.0.0-rc.10's prebuilt binary builds for aarch64-linux-android but
fails to load at runtime on RedMagic 11 Pro (Termux). Documented the
exact error, the workaround attempts, and the BM25-only fallback path.
Opens follow-up issue #<N>. BM25-only retrieval passes 100% strict
recall on the 91-query benchmark, so this is a graceful degradation,
not a regression.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
gh pr create --title "RedMagic 11: fastembed builds, ORT runtime broken — documented" --body "Outcome B of the Phase 0 fastembed probe. Follow-up issue #<N>."
```

Skip Task 13.

---

### Task 13: Outcome C — Doesn't build (only run if Task 10 → C)

- [ ] **Step 1: Append "Outcome C" section to the quickstart**

Append to `docs/devel/redmagic-termux-quickstart.md`:

```markdown
## Hybrid retrieval (`--embedder-backend fastembed`)

❌ Does not build on Android ARM64 as of 2026-05-26.

```
<exact build error captured in Task 10 Step 2>
```

Root cause: ORT 2.0.0-rc.10 has no prebuilt binary for
`aarch64-linux-android`, and the `cdn.pyke.io` download step fails at
build time. The workspace pins ORT at this exact version because the
vendored silero / whisper / piper crates require it; bumping ORT will
require a coordinated update to those vendor patches.

Use `--embedder-backend none` (BM25-only) on Android. The
retrieval-quality benchmark suite is tuned to pass at 100% strict recall
on the 91-query benchmark under BM25-only defaults, so this is a
graceful fallback, not a broken feature.

Follow-up issue: <link to opened GitHub issue>.
```

- [ ] **Step 2: Open the follow-up issue**

```bash
gh issue create \
    --title "fastembed/ORT cannot build for aarch64-linux-android (Termux)" \
    --label "android,embedding,blocked" \
    --body "$(cat <<'EOF'
ORT 2.0.0-rc.10 has no prebuilt binary for aarch64-linux-android on cdn.pyke.io. The build error on Termux (RedMagic 11 Pro, 2026-05-26) is:

```
<exact error>
```

The workspace pins ORT at this exact version because the vendored silero / whisper / piper crates require it (see CLAUDE.md). Unblocking this requires either:

(a) ORT's pyke.io bundle gaining an Android target — track upstream.
(b) Bumping ORT past rc.10 in lockstep with the speech vendor patches.
(c) System-installed `libonnxruntime.so` on Termux + ORT's `load-dynamic` feature — explored partially; if pursued, document.

Until resolved, `--embedder-backend none` (BM25-only) is the recommended Android setting; retrieval-quality benchmarks pass at 100% strict recall under BM25-only defaults on the 91-query corpus.
EOF
)"
```

- [ ] **Step 3: Update CLAUDE.md with the gap**

Add to CLAUDE.md (under the embedding-related conventions):

```markdown
- **Hybrid retrieval (`--embedder-backend fastembed`) is unavailable on Android ARM64** as of 2026-05-26 — ORT 2.0.0-rc.10 has no prebuilt binary for `aarch64-linux-android` and the build fails on Termux. See issue #<N>. BM25-only is the recommended Android setting; the retrieval-quality benchmark suite passes at 100% strict recall under BM25-only defaults so this is not a regression.
```

- [ ] **Step 4: Commit and open PR 2**

```bash
cd /Users/hherb/src/primer
git add docs/devel/redmagic-termux-quickstart.md CLAUDE.md
git commit -m "$(cat <<'EOF'
docs(android): hybrid retrieval — fastembed does not build on aarch64-linux-android

ORT 2.0.0-rc.10's cdn.pyke.io bundle has no Android target; --features embedding
fails at build time on Termux (RedMagic 11 Pro). Documented the exact failure,
unblocking paths, and the BM25-only fallback. Opens follow-up issue #<N>.
BM25-only retrieval passes 100% strict recall on the 91-query benchmark, so
this is a graceful degradation, not a regression.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
gh pr create --title "RedMagic 11: fastembed unavailable on Android — documented" --body "Outcome C of the Phase 0 fastembed probe. Follow-up issue #<N>."
```

---

## Verification matrix (across PR 1 + PR 2)

After both PRs land:

| Check | Where | Expected |
|---|---|---|
| `cargo build --bin primer` (default features) | Termux on RedMagic | success |
| `cargo test --workspace` (default features) | Termux on RedMagic | pass count ≥ dev-box - documented failures |
| 5-turn cloud REPL | Termux on RedMagic | conversation completes |
| 5-turn on-device Ollama REPL | Termux on RedMagic | conversation completes |
| Session DB + `--resume` | Termux on RedMagic | works |
| `android-cross-compile` CI job | GitHub Actions | green; required check after first clean run |
| Fastembed outcome | Termux on RedMagic | documented unambiguously (A / B / C) |
| Quickstart doc | `docs/devel/redmagic-termux-quickstart.md` | renders, has no `<…>` placeholders |
| Cross-refs from README + ROADMAP + CLAUDE.md | dev box | one line each pointing at quickstart |

## Self-review notes (for the implementer)

- Tasks 11, 12, 13 are mutually exclusive — only one runs based on Task 10's outcome.
- "Phone" tasks (2, 3, 4, 5, 10, 11/12/13 Step 1) require physical access to the RedMagic device. "Dev box" tasks can be done from the Mac in parallel where they don't depend on captured phone output.
- The CI cross-compile job is initially `continue-on-error: true` so the first push lands green. Task 9 Step 3 flips it after a clean run.
- If Task 4 surfaces a large platform-assumption fix (a whole module needs gating), defer it to a follow-up issue and document in CLAUDE.md rather than blowing up PR 1's scope.
- Frequent commits: one per logical chunk (workflow, doc, README batch). Don't squash everything into one mega-commit.
