# Hybrid retrieval default-on (CLI + GUI)

**Date:** 2026-05-30
**Status:** design — approved scope, pre-implementation
**Author:** Claude Code session (continuing from NEXT_SESSION 2026-05-30)

## Goal

Make hybrid retrieval (BM25 + dense-vector RRF via `fastembed`/BGE-M3) the
**default** for both the CLI (`primer`) and the GUI (`primer-gui`), instead of
the current BM25-only default. This realises the long-standing stated
preference recorded in CLAUDE.md:

> "The user's stated preference is for hybrid retrieval default-on once CI
> validates the cdn.pyke.io ort-runtime download; flip both
> `default = ["embedding"]` in `primer-cli/Cargo.toml` and the
> `--embedder-backend` default at that point."

The flip is gated on one prerequisite the NEXT_SESSION brief calls out: **CI
must prove the `cdn.pyke.io` ort-runtime download works on Linux AND macOS**
before hybrid becomes the path every default build takes.

## Background — the two coupled defaults

Hybrid retrieval is gated off at two independent layers today:

1. **Compile-time (cargo feature).** `primer-cli/Cargo.toml` and
   `primer-gui/Cargo.toml` both declare `default = []`. The `embedding`
   feature (which pulls `primer-embedding/fastembed` → `ort` → the BGE-M3
   ONNX model) is opt-in.
2. **Runtime (config default).** The CLI's `--embedder-backend` flag defaults
   to `"none"` (BM25-only). The GUI's `EmbedderConfig::default().kind` is
   `"none"`.

The **cdn.pyke.io download** is the **ort-runtime binary**, fetched by
`ort-sys`'s `build.rs` at *build* time when the `embedding`/`fastembed`
feature is active. (The ~570 MB BGE-M3 *model* is a separate *runtime*
download from HuggingFace, triggered by `FastEmbedBackend::new()`.) CI
already proves the ort download on **Linux** via
`cargo check -p primer-embedding --features fastembed` in the `feature-combos`
job; there is **no macOS equivalent** today.

**Critical interaction.** `build_fastembed_embedder` has a
`#[cfg(not(feature = "embedding"))]` arm that returns `Err(...)`; the CLI then
`std::process::exit(1)`s. So a *naive* flip of the runtime default to
`"fastembed"` would make a `--no-default-features` build hard-fail on a
flagless invocation. The design must avoid that.

## Non-goals

- **No new heavy CI download.** The full hybrid-recall test
  (`hybrid_retrieval_recall_with_fastembed`, which downloads ~570 MB BGE-M3
  and asserts recall) stays an on-demand `--features fastembed` test, NOT a CI
  gate. The acceptance criterion is "ort-runtime download works," not
  "full hybrid recall in CI on every push."
- **No change to the embedder backends themselves**, the RRF fusion, the
  knowledge-base schema, or the retrieval-quality tuning.
- **No migration of existing persisted configs.** An existing
  `gui-config.json` or `--embedder-backend` invocation keeps whatever the user
  explicitly chose; only the *default* (absent value / no flag) changes.

## Design

Four changes, each independently reviewable.

### 1. Feature-aware runtime default (the load-bearing correctness piece)

Rather than a naive flip to `"fastembed"`, make the runtime default **track
what is actually compiled in**:

- `#[cfg(feature = "embedding")]` → default `"fastembed"`
- `#[cfg(not(feature = "embedding"))]` → default `"none"`

This means:
- A **default build** (feature on) defaults to hybrid.
- A **`--no-default-features` build** (feature off) gracefully stays BM25-only
  instead of erroring on a flagless run.

This honours the codebase's existing philosophy — "fall back to BM25-only is
strictly better than refusing to start" (see `build_fastembed_embedder`'s doc
comment and the embedding-on-save fallback).

**CLI (`primer-cli/src/main.rs`).** clap's `default_value` must be a literal,
so use `cfg_attr` to pick the literal at compile time:

```rust
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

The doc comment above the field is updated to describe the feature-aware
default ("defaults to `fastembed` on a build with the `embedding` feature —
the default — and to `none` otherwise").

**GUI (`primer-gui/src/config.rs`).** `EmbedderConfig::default()` reads from a
cfg-gated free helper:

```rust
#[cfg(feature = "embedding")]
fn default_embedder_kind() -> &'static str { "fastembed" }
#[cfg(not(feature = "embedding"))]
fn default_embedder_kind() -> &'static str { "none" }

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

`#[serde(default)]` on the struct means an existing `gui-config.json` that
explicitly stored `"kind": "none"` keeps `"none"`; only a brand-new config
(or one missing the embedder block) gets the new feature-aware default. No
persisted-config migration is needed or wanted.

### 2. Flip the cargo defaults

- `primer-cli/Cargo.toml`: `default = []` → `default = ["embedding"]`.
- `primer-gui/Cargo.toml`: `default = []` → `default = ["embedding"]`.

Update the inline rationale comment in each (the `primer-cli` comment
currently says "Default-off pending CI validation… flip to
`default = ["embedding"]` once that's proven"); replace with a note that it is
now default-on and the CI proof lives in the Linux default-test job + the
macOS feature-combos job.

**Free Linux coverage.** Once `default = ["embedding"]`, the existing
`cargo test (default features)` job (ubuntu) builds the full workspace WITH
embedding, exercising the cdn.pyke.io ort download on Linux as part of normal
CI. No new Linux job needed.

### 3. Add the macOS ort-download proof

Add one step to the existing `feature-combos-macos` job (macos-latest),
matching that job's clippy convention:

```yaml
- name: cargo clippy -p primer-embedding --features fastembed
  # Proves the cdn.pyke.io ort-runtime download + link works on macOS,
  # the other half of the Linux proof in the feature-combos job. Closes
  # the prerequisite for flipping hybrid retrieval default-on.
  run: cargo clippy -p primer-embedding --features fastembed --all-targets -- -D warnings
```

`cargo clippy` (like `cargo check`) runs `build.rs`, which triggers the
cdn.pyke.io download — so this proves the download mechanism on macOS while
also catching lint rot (the macOS job's established pattern). This is the
build/check-level proof; running the model is out of scope per the non-goals.

### 4. Keep the Android cross-compile guard on `--no-default-features`

**This is a load-bearing consequence of flipping the cargo default, not an
optional extra.** The `android-cross-compile` job has two steps that build
`primer-cli` with its *default* features:

- `cargo build --target aarch64-linux-android --bin primer` (line ~371)
- `cargo build --target aarch64-linux-android -p primer-cli --features qnn`
  (line ~397; `--features` is additive to defaults)

Once `default = ["embedding"]`, both would pull `fastembed` → `ort-sys` →
attempt the **aarch64-linux-android ort-runtime download from cdn.pyke.io**,
which CLAUDE.md / issue #157 document as **device-unverified** (the cfg patch
is verified; whether `build.rs` can fetch a prebuilt android ORT binary is
not). That would risk breaking the required cross-compile guard for a path the
product explicitly does not ship by default on Android.

Fix: pin both steps to `--no-default-features`:

- `cargo build --target aarch64-linux-android --no-default-features --bin primer`
- `cargo build --target aarch64-linux-android --no-default-features -p primer-cli --features qnn`

This keeps the Android guard testing exactly the documented Android build —
BM25-only, no ort — matching the "conservative Android setting remains
`--embedder-backend none`" guidance. `primer-cli` has no *other* default
feature today, so `--no-default-features` drops only `embedding`; the binary
still builds with the stub/cloud/ollama/openai-compat backends (those are not
cargo features). The `qnn_bench` example step (`-p primer-inference`) is
unaffected — `primer-inference` has no `embedding` feature in its graph (to be
re-confirmed during implementation).

## Test impact

- **CLI:** No existing test asserts `embedder_backend == "none"`; the only
  reference is the `default_value` attribute itself. Add a small unit test
  (gated `#[cfg(feature = "embedding")]`) asserting `Cli::parse_from(["primer"])`
  yields `embedder_backend == "fastembed"`, and the inverse under
  `#[cfg(not(feature = "embedding"))]`. (TDD: write these first.)
- **GUI:** `config.rs:755` compares `EmbedderConfig::default()` to itself —
  robust to the change. The `"kind": "none"` JSON fixtures (lines 919, 1000,
  1188) test *explicit* deserialization and are unaffected. Add a feature-aware
  unit test pinning `EmbedderConfig::default().kind` (`"fastembed"` with the
  feature, `"none"` without).
- **No retrieval-quality test changes.** Those are already keyed off explicit
  embedder construction, not the default.

## Docs to update

- **README.md** — the user-facing status line that describes hybrid as
  "opt-in via `--embedder-backend`" flips to "default-on (BM25-only via
  `--embedder-backend none` or a `--no-default-features` build)".
- **ROADMAP.md** — note hybrid default-on shipped.
- **CLAUDE.md** — update the "Hybrid retrieval is opt-in" gotcha to
  "default-on"; record the feature-aware-default pattern and the macOS CI
  proof. Update the `--embedder-backend` flag description.

## Risks

- **Default CI build time + first-run download.** The default Linux test job
  now compiles `ort` + `fastembed` for both `primer-cli` and `primer-gui` and
  downloads the ort runtime on a cold cache. The `Swatinem/rust-cache` layer
  keeps subsequent runs warm; first run after the flip is slower. Acceptable —
  this is the cost of making hybrid the default and is exactly what the CI
  proof is meant to exercise.
- **First-run UX for end users.** A fresh default build now downloads ~570 MB
  BGE-M3 on first conversation (with the existing "Loading fastembed model…"
  banner) and falls back to BM25-only + a `tracing::warn!` if the download
  fails. This is the intended behaviour; the fallback means the conversation
  still works offline/on failure.
- **Android default stays `none`.** Per CLAUDE.md / issue #157, the
  conservative Android setting remains `--embedder-backend none` until on-device
  Termux ort-download validation lands. Because flipping the cargo default
  WOULD otherwise drag the unverified android ort download into the
  cross-compile guard, change #4 pins the Android job to
  `--no-default-features` — keeping the Android build BM25-only and the guard
  green. The docs are updated to re-note that Android remains BM25-only by
  guidance.
- **`macos-latest` runner OS.** The macOS proof runs on whatever macOS image
  GitHub provides for `macos-latest`; the ort rc.10 binary must exist for that
  arch on cdn.pyke.io. If the download 404s for the runner's arch, the new step
  fails loudly — which is the correct signal (it means macOS hybrid isn't
  actually supported yet), and we'd reassess rather than ship a false default.

## Acceptance criteria

1. `primer-cli` and `primer-gui` both declare `default = ["embedding"]`.
2. A default `cargo run --bin primer` (no flags) runs hybrid retrieval
   (constructs a `FastEmbedBackend`, or falls back to BM25-only with a warn on
   download failure) — verified by the feature-aware default test.
3. A `cargo run --bin primer --no-default-features` (no flags) runs BM25-only
   without erroring — verified by the inverse default test.
4. CI proves the cdn.pyke.io ort-runtime download on **both** Linux (existing
   default-test job, now building embedding) and macOS (new feature-combos-macos
   step).
5. The Android cross-compile job stays green: both `primer-cli` steps build
   `--no-default-features` (BM25-only), so the flip does not drag the
   device-unverified android ort download into the required guard.
6. `cargo clippy --workspace --all-targets -- -D warnings` and
   `cargo fmt --all -- --check` clean.
7. README / ROADMAP / CLAUDE.md updated.
