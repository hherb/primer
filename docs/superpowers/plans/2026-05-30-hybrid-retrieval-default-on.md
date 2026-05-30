# Hybrid Retrieval Default-On (CLI + GUI) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make hybrid retrieval (BM25 + dense-vector RRF via fastembed/BGE-M3) the default for the CLI and GUI, with a feature-aware runtime default that gracefully degrades to BM25-only on `--no-default-features` builds, and CI that proves the cdn.pyke.io ort-runtime download on Linux + macOS.

**Architecture:** Two coupled defaults flip together per crate: the `embedding` cargo feature moves into `default = [...]`, and the runtime embedder-backend default becomes `fastembed` *only when that feature is compiled in* (via `cfg_attr` on the CLI clap arg and a cfg-gated helper for the GUI config). CI gets a macOS ort-download proof step; the Android cross-compile guard is pinned to `--no-default-features` so the flip doesn't drag the device-unverified android ort download into the required check.

**Tech Stack:** Rust (edition 2024, toolchain 1.88), clap derive, serde, GitHub Actions. All cargo commands run from `src/` using `~/.cargo/bin/cargo` (rustup proxy — avoids Homebrew rust shadowing per CLAUDE.md).

**Spec:** [docs/superpowers/specs/2026-05-30-hybrid-retrieval-default-on-design.md](../specs/2026-05-30-hybrid-retrieval-default-on-design.md)

---

## File Structure

- **Modify:** `src/crates/primer-cli/src/main.rs` — feature-aware `cfg_attr` on the `embedder_backend` clap arg + doc comment; new parse tests in the existing `break_suggest_flag_tests`-style test module.
- **Modify:** `src/crates/primer-cli/Cargo.toml` — `default = []` → `default = ["embedding"]` + comment.
- **Modify:** `src/crates/primer-gui/src/config.rs` — cfg-gated `default_embedder_kind()` helper + `EmbedderConfig::default()`; new feature-aware default test.
- **Modify:** `src/crates/primer-gui/Cargo.toml` — `default = []` → `default = ["embedding"]` + comment.
- **Modify:** `.github/workflows/ci.yml` — add macOS fastembed proof step to `feature-combos-macos`; pin two Android `primer-cli` steps to `--no-default-features`.
- **Modify:** `README.md`, `ROADMAP.md`, `CLAUDE.md` — flip "opt-in" → "default-on"; document feature-aware default + macOS CI proof + Android-stays-BM25 guidance.

> **Note on test cost:** building with `embedding` on (now the default) compiles `ort` + `fastembed` and downloads the ort runtime from cdn.pyke.io on a cold cache (one-time, ~minutes). This is expected and is exactly what the CI proof exercises.

---

## Task 1: CLI feature-aware runtime default

**Files:**
- Modify: `src/crates/primer-cli/src/main.rs` (arg at lines ~204-214; new test module near line ~1598)

- [ ] **Step 1: Write the failing tests**

Append a new test module at the end of `src/crates/primer-cli/src/main.rs` (after the `break_suggest_flag_tests` module, ~line 1598):

```rust
#[cfg(test)]
mod embedder_backend_default_tests {
    use super::*;
    use clap::Parser;

    /// On a build with the `embedding` feature (the default), a flagless
    /// invocation defaults to hybrid retrieval via fastembed.
    #[cfg(feature = "embedding")]
    #[test]
    fn default_is_fastembed_with_embedding_feature() {
        let cli = Cli::try_parse_from(["primer", "--name", "Ada", "--age", "9"]).unwrap();
        assert_eq!(cli.embedder_backend, "fastembed");
    }

    /// On a `--no-default-features` build (embedding off), the default
    /// stays BM25-only so the binary never hard-fails on a flagless run.
    #[cfg(not(feature = "embedding"))]
    #[test]
    fn default_is_none_without_embedding_feature() {
        let cli = Cli::try_parse_from(["primer", "--name", "Ada", "--age", "9"]).unwrap();
        assert_eq!(cli.embedder_backend, "none");
    }

    /// An explicit value always overrides the default, both ways.
    #[test]
    fn explicit_value_overrides_default() {
        let cli = Cli::try_parse_from([
            "primer", "--name", "Ada", "--age", "9", "--embedder-backend", "none",
        ])
        .unwrap();
        assert_eq!(cli.embedder_backend, "none");
    }
}
```

- [ ] **Step 2: Run the embedding-arm test to verify it FAILS**

Run (from `src/`): `~/.cargo/bin/cargo test -p primer-cli --features embedding embedder_backend_default`
Expected: `default_is_fastembed_with_embedding_feature` FAILS — the arg still defaults to `"none"` (assertion: left `"none"`, right `"fastembed"`). `explicit_value_overrides_default` passes. (First run downloads the ort runtime.)

- [ ] **Step 3: Apply the feature-aware `cfg_attr` and update the doc comment**

In `src/crates/primer-cli/src/main.rs`, replace the doc comment + arg (lines ~204-214):

```rust
    /// Embedder backend for hybrid retrieval. Defaults to `fastembed` on a
    /// build with the `embedding` cargo feature (the default build) and to
    /// `none` on a `--no-default-features` build — so a flagless run does
    /// the right thing for whatever was compiled in and never hard-fails.
    /// `none` disables hybrid retrieval and uses BM25-only; `stub` uses the
    /// in-process deterministic hash embedder (no semantic value, only
    /// useful for testing the hybrid pipeline end-to-end); `fastembed` uses
    /// the BGE-M3 dense embedding model via `fastembed-rs` (~570 MB on first
    /// run; requires the `embedding` cargo feature); `ollama` uses Ollama's
    /// `/api/embeddings` (requires the `ollama-embedding` cargo feature and
    /// Ollama running locally); `openai-compat` uses a `/v1/embeddings`
    /// server (requires the `openai-compat-embedding` cargo feature).
    #[cfg_attr(
        feature = "embedding",
        arg(long, value_name = "BACKEND", default_value = "fastembed")
    )]
    #[cfg_attr(
        not(feature = "embedding"),
        arg(long, value_name = "BACKEND", default_value = "none")
    )]
    embedder_backend: String,
```

- [ ] **Step 4: Run the test to verify it PASSES**

Run (from `src/`): `~/.cargo/bin/cargo test -p primer-cli --features embedding embedder_backend_default`
Expected: all three tests PASS.

- [ ] **Step 5: Verify the no-feature arm too**

Run (from `src/`): `~/.cargo/bin/cargo test -p primer-cli --no-default-features embedder_backend_default`
Expected: `default_is_none_without_embedding_feature` + `explicit_value_overrides_default` PASS (the fastembed test is cfg'd out).

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-cli/src/main.rs
git commit -m "feat(cli): feature-aware --embedder-backend default (fastembed when embedding on)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Flip the CLI cargo default to `embedding`

**Files:**
- Modify: `src/crates/primer-cli/Cargo.toml` (the `[features]` block, `default` line + comment at lines ~44-53)

- [ ] **Step 1: Flip the default and rewrite the rationale comment**

In `src/crates/primer-cli/Cargo.toml`, replace:

```toml
[features]
default = []

# Hybrid retrieval via the fastembed-rs (BGE-M3) backend.
# Default-off pending CI validation of the cdn.pyke.io ort-runtime
# download path; flip to `default = ["embedding"]` once that's proven.
```

with:

```toml
[features]
# Hybrid retrieval is default-on (BM25 + dense-vector RRF via BGE-M3).
# The cdn.pyke.io ort-runtime download is proven in CI on Linux (the
# default `cargo test` job now builds embedding) and macOS (the
# feature-combos-macos job's fastembed step). A `--no-default-features`
# build drops to BM25-only and the runtime default tracks that via the
# `cfg_attr` on `--embedder-backend` in main.rs. Android stays BM25-only
# by guidance (the cross-compile guard pins `--no-default-features`; see
# CLAUDE.md / issue #157).
default = ["embedding"]

# Hybrid retrieval via the fastembed-rs (BGE-M3) backend.
```

(Leave the rest of the `embedding = [...]` comment lines about the ort/fastembed pin intact.)

- [ ] **Step 2: Verify the default build now defaults to fastembed**

Run (from `src/`): `~/.cargo/bin/cargo test -p primer-cli embedder_backend_default`
Expected: `default_is_fastembed_with_embedding_feature` + `explicit_value_overrides_default` PASS (now without `--features`, because embedding is default). The `--no-default-features` variant is still covered by Task 1 Step 5.

- [ ] **Step 3: Verify `--help` reflects the new default**

Run (from `src/`): `~/.cargo/bin/cargo run -q --bin primer -- --help | grep -A1 embedder-backend`
Expected: the help text shows `[default: fastembed]`.

- [ ] **Step 4: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-cli/Cargo.toml
git commit -m "feat(cli): flip default = [\"embedding\"] — hybrid retrieval default-on

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: GUI feature-aware embedder default

**Files:**
- Modify: `src/crates/primer-gui/src/config.rs` (`EmbedderConfig::default()` at lines ~257-266; new test in the `tests` module at line ~641)

- [ ] **Step 1: Write the failing tests**

In `src/crates/primer-gui/src/config.rs`, inside `mod tests` (after the `partial_json_fills_unspecified_fields_with_defaults` test, ~line 757), add:

```rust
    #[cfg(feature = "embedding")]
    #[test]
    fn embedder_default_is_fastembed_with_feature() {
        assert_eq!(EmbedderConfig::default().kind, "fastembed");
    }

    #[cfg(not(feature = "embedding"))]
    #[test]
    fn embedder_default_is_none_without_feature() {
        assert_eq!(EmbedderConfig::default().kind, "none");
    }
```

- [ ] **Step 2: Run the embedding-arm test to verify it FAILS**

Run (from `src/`): `~/.cargo/bin/cargo test -p primer-gui --features embedding embedder_default`
Expected: `embedder_default_is_fastembed_with_feature` FAILS (default `kind` is still `"none"`). (First run downloads the ort runtime; the GUI build is heavier — Tauri + ort.)

- [ ] **Step 3: Add the cfg-gated helper and use it in `Default`**

In `src/crates/primer-gui/src/config.rs`, replace the `EmbedderConfig` `Default` impl (lines ~257-266):

```rust
impl Default for EmbedderConfig {
    fn default() -> Self {
        Self {
            kind: "none".to_string(),
            model: None,
            ollama_url: None,
            openai_compat_url: None,
        }
    }
}
```

with:

```rust
/// The default embedder kind tracks what is compiled in: a build with the
/// `embedding` feature (the default) defaults to hybrid retrieval via
/// fastembed; a `--no-default-features` build stays BM25-only so the GUI
/// never refuses to start. `#[serde(default)]` on the config means an
/// existing `gui-config.json` that explicitly stored `"none"` keeps it —
/// only a fresh config picks up this default.
#[cfg(feature = "embedding")]
fn default_embedder_kind() -> &'static str {
    "fastembed"
}

#[cfg(not(feature = "embedding"))]
fn default_embedder_kind() -> &'static str {
    "none"
}

impl Default for EmbedderConfig {
    fn default() -> Self {
        Self {
            kind: default_embedder_kind().to_string(),
            model: None,
            ollama_url: None,
            openai_compat_url: None,
        }
    }
}
```

- [ ] **Step 4: Run the test to verify it PASSES**

Run (from `src/`): `~/.cargo/bin/cargo test -p primer-gui --features embedding embedder_default`
Expected: `embedder_default_is_fastembed_with_feature` PASSES.

- [ ] **Step 5: Verify the no-feature arm and that existing default-equality test still holds**

Run (from `src/`): `~/.cargo/bin/cargo test -p primer-gui --no-default-features embedder_default partial_json_fills`
Expected: `embedder_default_is_none_without_feature` + `partial_json_fills_unspecified_fields_with_defaults` PASS. (The latter compares `EmbedderConfig::default()` to itself, so it holds either way.)

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-gui/src/config.rs
git commit -m "feat(gui): feature-aware EmbedderConfig default (fastembed when embedding on)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Flip the GUI cargo default to `embedding`

**Files:**
- Modify: `src/crates/primer-gui/Cargo.toml` (the `[features]` block `default` line + comment)

- [ ] **Step 1: Flip the default and add a rationale comment**

In `src/crates/primer-gui/Cargo.toml`, replace:

```toml
[features]
default = []
```

with:

```toml
[features]
# Hybrid retrieval is default-on, mirroring primer-cli. A fresh
# `gui-config.json` defaults the embedder to fastembed (see
# EmbedderConfig::default in config.rs); an existing config keeps whatever
# the user stored. A `--no-default-features` build drops to BM25-only.
default = ["embedding"]
```

- [ ] **Step 2: Verify the default GUI build now defaults to fastembed**

Run (from `src/`): `~/.cargo/bin/cargo test -p primer-gui embedder_default`
Expected: `embedder_default_is_fastembed_with_feature` PASSES without `--features` (embedding is now default).

- [ ] **Step 3: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-gui/Cargo.toml
git commit -m "feat(gui): flip default = [\"embedding\"] — hybrid retrieval default-on

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: CI — macOS ort-download proof + Android `--no-default-features`

**Files:**
- Modify: `.github/workflows/ci.yml` (`feature-combos-macos` job ~line 222; `android-cross-compile` steps at lines ~371 and ~397)

- [ ] **Step 1: Add the macOS fastembed proof step**

In `.github/workflows/ci.yml`, in the `feature-combos-macos` job, after the last existing `cargo clippy -p primer-cli --features speech,macos-native` step (~line 275), add:

```yaml
      - name: cargo clippy -p primer-embedding --features fastembed
        # Proves the cdn.pyke.io ort-runtime download + link works on macOS
        # — the other half of the Linux proof in the feature-combos job.
        # `clippy` (like `check`) runs build.rs, which triggers the download.
        # This is the prerequisite that gated flipping hybrid retrieval
        # default-on (see docs/superpowers/specs/2026-05-30-hybrid-retrieval-default-on-design.md).
        run: cargo clippy -p primer-embedding --features fastembed --all-targets -- -D warnings
```

- [ ] **Step 2: Pin the Android `primer-cli` steps to `--no-default-features`**

In `.github/workflows/ci.yml`, in the `android-cross-compile` job:

Change the "Cross-compile primer binary" step (~line 371) from:

```yaml
        run: cargo build --target aarch64-linux-android --bin primer
```

to:

```yaml
        # `--no-default-features`: hybrid retrieval is default-on for the
        # host CLI, but the aarch64-linux-android ort-runtime download from
        # cdn.pyke.io is device-unverified (issue #157). The Android product
        # ships BM25-only by guidance (`--embedder-backend none`), so the
        # cross-compile guard builds without the `embedding` feature.
        run: cargo build --target aarch64-linux-android --no-default-features --bin primer
```

Change the "Cross-compile primer-cli --features qnn" step (~line 397) from:

```yaml
        run: cargo build --target aarch64-linux-android -p primer-cli --features qnn
```

to:

```yaml
        # `--no-default-features --features qnn`: `--features` is additive to
        # defaults, so without `--no-default-features` this would also pull
        # the default `embedding` feature → the device-unverified android ort
        # download (issue #157). Keep the qnn guard BM25-only.
        run: cargo build --target aarch64-linux-android --no-default-features -p primer-cli --features qnn
```

(Leave the `qnn_bench` example step at ~line 409 unchanged — it builds `-p primer-inference`, which has no `embedding` feature.)

- [ ] **Step 3: Validate the workflow YAML parses**

Run (from repo root): `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('YAML OK')"`
Expected: `YAML OK`.

- [ ] **Step 4: Sanity-check the Android no-default-features build locally (if the android target is installed)**

Run (from `src/`): `~/.cargo/bin/cargo build --target aarch64-linux-android --no-default-features --bin primer 2>&1 | tail -5`
Expected: builds (or, if the NDK/target isn't installed locally, a clear toolchain error — NOT a feature/embedding error). If the target isn't installed, skip this step; CI is the authority.

- [ ] **Step 5: Commit**

```bash
cd /Users/hherb/src/primer
git add .github/workflows/ci.yml
git commit -m "ci: macOS fastembed ort-download proof + pin android guard to --no-default-features

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Documentation

**Files:**
- Modify: `README.md` (lines ~45 and ~310-312), `ROADMAP.md` (line ~24), `CLAUDE.md` (line ~159)

- [ ] **Step 1: Update README — feature bullet (line ~45)**

In `README.md`, in the "Hybrid retrieval" bullet (~line 45), replace:

```
Opt-in via `--embedder-backend none|stub|fastembed|ollama` (default `none`); `fastembed` uses BGE-M3 (1024-dim multilingual, ~570 MB on first run) behind the `embedding` cargo feature.
```

with:

```
Default-on via `--embedder-backend` (`fastembed` on a default build, `none` on a `--no-default-features` build); `fastembed` uses BGE-M3 (1024-dim multilingual, ~570 MB downloaded on first run, falling back to BM25-only with a warning if the download fails). Pass `--embedder-backend none` for BM25-only. The `embedding` cargo feature is now in the default set. Android ships BM25-only by guidance (issue #157).
```

- [ ] **Step 2: Update README — CLI help block (lines ~310-312)**

In `README.md`, in the CLI flags block, replace the `--embedder-backend` default note:

```
                                (default: none = BM25-only, the pre-Phase-0.2.5 behaviour). `stub`
```

with:

```
                                (default: fastembed on a default build = hybrid; none on a
                                --no-default-features build = BM25-only). `stub`
```

- [ ] **Step 3: Update ROADMAP (line ~24)**

In `ROADMAP.md`, change the hybrid-retrieval bullet (~line 24) ending from:

```
Falls back to BM25-only when no embedder is wired. Opt-in via `--embedder-backend`.
```

to:

```
Falls back to BM25-only when no embedder is wired. Default-on via `--embedder-backend` (feature-aware: `fastembed` on a default build, `none` on `--no-default-features`); the cdn.pyke.io ort-runtime download is proven in CI on Linux + macOS.
```

- [ ] **Step 4: Update CLAUDE.md gotcha (line ~159)**

In `CLAUDE.md`, replace the opening of the hybrid-retrieval gotcha (~line 159):

```
- **Hybrid retrieval is opt-in via the `embedding` cargo feature AND the `--embedder-backend` flag.** Two layers: (1) compile-time, the `embedding` cargo feature decides whether `FastEmbedBackend` is built at all; (2) runtime, `--embedder-backend none|stub|fastembed|ollama` (default `none`) decides which embedder the dialogue manager uses. `none` is BM25-only — exactly the pre-Phase-0.2.5 behaviour. `stub` constructs a deterministic FNV-hash embedder useful only for testing the hybrid pipeline structurally; in production it dilutes BM25 with semantic noise and should not be the default. The user's stated preference is for hybrid retrieval default-on once CI validates the cdn.pyke.io ort-runtime download; flip both `default = ["embedding"]` in `primer-cli/Cargo.toml` and the `--embedder-backend` default at that point. Without the feature, an explicit `--embedder-backend fastembed` exits with a build hint.
```

with:

```
- **Hybrid retrieval is default-on, with a feature-aware default that degrades gracefully.** Two layers: (1) compile-time, the `embedding` cargo feature decides whether `FastEmbedBackend` is built — it is now in `default = ["embedding"]` for BOTH `primer-cli` and `primer-gui`; (2) runtime, the `--embedder-backend` default (and the GUI's `EmbedderConfig::default().kind`) is **feature-aware** — `fastembed` when the `embedding` feature is compiled in (the default build), `none` on a `--no-default-features` build. This is the load-bearing trick that lets a flagless run do the right thing for whatever was compiled in without ever hard-failing: the CLI uses `#[cfg_attr(feature = "embedding", arg(... default_value = "fastembed"))]` + a `not(...)` arm; the GUI uses a cfg-gated `default_embedder_kind()` helper. `none` is BM25-only; `stub` constructs a deterministic FNV-hash embedder useful only for testing the hybrid pipeline structurally (dilutes BM25 with noise in production). The cdn.pyke.io ort-runtime download is proven in CI on Linux (the default `cargo test` job now builds embedding) and macOS (the `feature-combos-macos` job's `cargo clippy -p primer-embedding --features fastembed` step). **Android stays BM25-only by guidance** (issue #157): the `android-cross-compile` job pins both `primer-cli` steps to `--no-default-features` so the flip doesn't drag the device-unverified aarch64 ort download into the required guard; runtime guidance remains `--embedder-backend none` on Android. Without the feature, an explicit `--embedder-backend fastembed` still exits with a build hint.
```

- [ ] **Step 5: Verify docs-only changes don't break anything + final full verification**

Run (from `src/`):
```bash
~/.cargo/bin/cargo fmt --all -- --check
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo test --workspace --no-fail-fast
```
Expected: fmt clean; clippy clean; all tests pass (the workspace now builds with embedding by default, so the first run downloads the ort runtime once).

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer
git add README.md ROADMAP.md CLAUDE.md
git commit -m "docs: hybrid retrieval default-on (CLI + GUI); android stays BM25-only by guidance

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final verification checklist (acceptance criteria)

- [ ] `primer-cli` and `primer-gui` both declare `default = ["embedding"]`.
- [ ] `cargo test -p primer-cli embedder_backend_default` (default features) → fastembed default test passes.
- [ ] `cargo test -p primer-cli --no-default-features embedder_backend_default` → none default test passes (no error).
- [ ] `cargo test -p primer-gui embedder_default` (default features) → fastembed default test passes.
- [ ] CI: `feature-combos-macos` has a `cargo clippy -p primer-embedding --features fastembed` step; `android-cross-compile` builds `primer-cli` with `--no-default-features`.
- [ ] `cargo fmt --all -- --check` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace` clean.
- [ ] README / ROADMAP / CLAUDE.md updated.
