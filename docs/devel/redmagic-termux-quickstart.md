# RedMagic 11 — Phase 0 quickstart (Termux)

This guide gets the Primer's Phase 0 text REPL running on a RedMagic 11 Pro
(Snapdragon 8 Elite, 24 GB RAM) inside Termux, end-to-end. Validated
2026-05-26 on a stock device — see "What works, what doesn't" at the end
for the honest current state, including the load-bearing finding that
**on-device CPU inference is too slow for conversational use** and the
phone is dependent on cloud or a future NPU backend.

The companion design lives at [docs/superpowers/specs/2026-05-26-redmagic-termux-port-design.md](../superpowers/specs/2026-05-26-redmagic-termux-port-design.md);
the implementation plan at [docs/superpowers/plans/2026-05-26-redmagic-termux-port.md](../superpowers/plans/2026-05-26-redmagic-termux-port.md).

## Prereqs

- Termux installed **from F-Droid**, not Play Store. The Play Store
  version's package manager has been broken for years.
- Storage permission granted (Android settings → Apps → Termux →
  Permissions, or `termux-setup-storage` after install).
- An Anthropic API key (for the cloud backend smoke — this is the
  recommended Phase 0 setup on RedMagic; see the inference-perf note
  below for why local Ollama is not yet usable).

## Install build prereqs

```bash
pkg update -y && pkg upgrade -y
pkg install -y rust clang make pkg-config openssl-tool git
rustc --version    # expect ≥ 1.88 (the workspace pin in src/rust-toolchain.toml)
```

If pkg's `rustc` is older than 1.88, install rustup so the workspace's
pinned-toolchain file is honored:

```bash
pkg install -y rustup
rustup default stable
rustc --version    # should now be ≥ 1.88
```

Grant Termux storage access if you haven't already:

```bash
termux-setup-storage
```

This creates `~/storage/shared/ → /sdcard/`, which you'll want for any
later file transfer off the device (e.g. log capture for the project
maintainers — see "Capturing a session" below).

## Clone and build

```bash
cd ~
git clone https://github.com/hherb/primer.git
cd primer/src     # NB: the workspace root is src/, not the repo root
cargo build --bin primer
```

The build takes a while on first run (lots of dependencies to compile
from source). `cargo build --bin primer` builds only the dependency
graph for the `primer` binary — it does NOT build `primer-gui` (Tauri
desktop), `primer-speech` (the speech feature is off by default), or
`primer-kb-load` as a binary.

> **Empirical surprise:** `cargo test --workspace` did successfully
> compile `primer-gui` to a test binary in our run, despite Tauri 2.x's
> webkit2gtk dep being unix-host-only. Either Termux's repos quietly
> supply a stub or Tauri's test profile links differently from release.
> Either way: more permissive than expected. Don't rely on it — the
> documented path is `--bin primer` only.

## Smoke 1: stub backend (no network, no API key)

```bash
echo "hi" | cargo run --bin primer
```

The Primer responds with a canned Socratic line. This confirms the
binary works end-to-end without any external dependency. Type `quit`
to exit if you start an interactive session.

## Smoke 2: cloud backend (Anthropic) — recommended Phase 0 setup

```bash
echo 'ANTHROPIC_API_KEY=sk-ant-...' >> ~/.primer_env
chmod 600 ~/.primer_env
cd ~/primer/src
cargo run --bin primer -- --backend cloud --name TestChild --age 8
```

This is the load-bearing Phase 0 setup. Sessions persist locally to
`~/.primer/testchild.db` (slug derived from `--name`). The full
auto-seeded knowledge base loads at startup:

```
INFO primer-kb-load: loading seed corpus … path=…/data/seed/seed_passages.en.jsonl
INFO primer-kb-load: loaded JSONL into knowledge base inserted=56 skipped=0 sources=56
INFO primer-kb-load: loading seed corpus … path=…/data/seed/wiki_passages.en.jsonl
INFO primer-kb-load: loaded JSONL into knowledge base inserted=35 skipped=0 sources=35
INFO primer: auto-seeded knowledge base for locale en inserted=91 sources=91
```

Engagement, concept-extraction, and comprehension classifiers all wire
through to the same cloud backend by default:

```
Engagement classifier: llm:claude-sonnet-4-6
Concept extractor: llm:cloud-anthropic:claude-sonnet-4-6
Comprehension classifier: llm:cloud-anthropic:claude-sonnet-4-6
```

Type `quit` to exit. A first-run banner explains the persistence path.

### Resume a prior session

```bash
sqlite3 ~/.primer/testchild.db \
    'SELECT id FROM sessions ORDER BY started_at DESC LIMIT 1;'
# Copy the UUID, then:
cargo run --bin primer -- --backend cloud \
    --name TestChild \
    --resume <uuid>
```

> **Gotcha:** `--resume` reads from `--session-db`, which defaults to
> `~/.primer/<slug-of-name>.db`. If you change `--name` (or drop it,
> defaulting to `Explorer`) on the resume command, the CLI looks in
> the wrong DB file and errors with `no session with id X found in
> /…/explorer.db`. Pass the same `--name` you used on the original
> session, OR pass `--session-db ~/.primer/testchild.db` explicitly.

## Smoke 3: on-device Ollama backend

```bash
ollama list    # confirm Ollama is up and pick a model
cargo run --bin primer -- \
    --backend ollama --model <model-name> \
    --name TestChild2 --age 8 \
    --ollama-url http://localhost:11434
```

> **Critical perf finding (2026-05-26):** With Ollama running on the
> RedMagic 11 Pro's CPU only (Termux's Ollama package has no Vulkan/QNN
> path), **a 4-bit-quantised 4B model is too slow for conversational
> Socratic dialogue.** Token-rate is fine for general chat but
> insufficient for the kind of back-and-forth a child needs to stay
> engaged. The spec called this out as the "phone-as-Primer viability"
> question; the empirical answer is that on-device inference on
> current-generation Snapdragon 8 Elite hardware **requires NPU
> acceleration** to be conversational. That is ROADMAP Phase 1.2 (QNN
> backend), and it's load-bearing for the standalone-phone product
> story, not optional.

For now the cloud backend (Smoke 2) is the recommended Phase 0 setup
on this device.

> **Gotcha (reasoning-mode models):** Models that emit chain-of-thought
> reasoning (DeepSeek-R1, Gemma "thinking" variants, Qwen QwQ,
> `medgemma1.5`, etc.) leak their internal reasoning tokens into the
> visible response — verbatim, child-facing. This is pedagogically the
> opposite of the Socratic method. The Primer's `OllamaBackend` does not
> currently strip these markers (`<think>…</think>`, `<unused94>thought`
> etc. depending on the model). **Use a base instruction-tuned model
> for now;** proper stream-aware handling is a follow-up issue.

## Where your child's data lives

- Per-learner session DB: `~/.primer/<slug>.db` →
  `/data/data/com.termux/files/home/.primer/<slug>.db` on Android.
- Long-term memory + summaries + FTS5 retrieval index: same DB
  (storage schema v8 at the time of this writing).
- Knowledge base: in-memory (`:memory:`) by default; pass
  `--knowledge-db <path>` to persist.

Per [CLAUDE.md](../../CLAUDE.md) and roadmap principle #3: all learner
data stays local. Cloud inference is stateless — only per-turn messages
travel over the wire, never the persisted learner model.

## Capturing a session for the maintainers

If you want to pass a full transcript back (debugging, regression
reporting, contributing to the doc):

```bash
pkg install -y util-linux            # for the `script` command
script -a ~/storage/shared/primer-session.log
# ... do work in this sub-shell, including cargo runs ...
exit                                 # cleanly flushes the log
```

`script` records the whole pty session including ANSI control codes;
`cat session.log | col -bp > session.clean.log` strips most of the
noise. The `~/storage/shared/` path puts the file on the device's
shared storage so you can pull it off via SSH+scp (see below),
LocalSend, or USB.

To pull off via SSH (which Termux ships):

```bash
# On phone:
pkg install -y openssh
passwd                                # set a password
sshd                                  # start server on port 8022
ifconfig wlan0 | grep "inet "         # capture phone's WiFi IP
whoami                                # capture termux username

# On dev box:
scp -P 8022 <termux-user>@<phone-ip>:/data/data/com.termux/files/home/storage/shared/primer-session.log ~/Desktop/
```

`ssh` uses lowercase `-p` for the port number; `scp` uses uppercase
`-P`. Historical mistake nobody can fix.

## Troubleshooting

### `tee: /tmp/foo.log: Permission denied` (or similar `/tmp` writes)

`/tmp` is **not writable** in Termux's Android sandbox. Use `$TMPDIR`
(Termux sets it to a writable per-app temp dir) or just `~/`:

```bash
cargo test --workspace --no-fail-fast 2>&1 | tee ~/cargo-test.log
# or
cargo build 2>&1 | tee $TMPDIR/cargo-build.log
```

### `error[E0463]: can't find crate for core` during cargo build

The Android target needs to be installed for the workspace's pinned
toolchain (1.88), not just the default toolchain. After
`rustup default stable`, the target install applies automatically; if
you used `pkg install rust` and then added the target manually, run:

```bash
rustup target add aarch64-linux-android --toolchain 1.88
```

(For on-device builds you don't need the Android target at all — that
target is for cross-compiling FROM a desktop. On-device builds use
`aarch64-linux-android` as the host target, which `pkg install rust`
already provides.)

### `cargo test --workspace` looks hung mid-run

Almost certainly not hung — phone CPUs are slow and there are 850+
tests across the workspace. In a second Termux tab:

```bash
ls -la ~/cargo-test.log     # mtime updates → progress
tail -f ~/cargo-test.log    # watch live
ps aux | grep cargo         # confirms cargo + test binary active
```

If you want live per-test output: `--test-threads=1 --nocapture`.

### `Error: no session with id X found in /…/explorer.db`

You ran `--resume` without matching `--name` or `--session-db` to the
original session — the CLI defaulted the session DB path to a different
file. See the "Gotcha" callout under "Resume a prior session" above.

## Hybrid retrieval (`--embedder-backend fastembed`)

❌ **Does not build on Android ARM64** as of 2026-05-27.

```bash
cargo build -p primer-cli --features embedding
```

fails at the `ort-sys` build script:

```
error[E0432]: unresolved import `self::internal::dirs::cache_dir`
   --> ort-sys-2.0.0-rc.10/build.rs:26:5
   |
26 | use self::internal::dirs::cache_dir;
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ no `cache_dir` in `internal::dirs`
   |
note: found an item that was configured out
   --> ort-sys-2.0.0-rc.10/src/internal/dirs.rs:177:8
   |
175 | #[cfg(target_os = "linux")]
176 | #[must_use]
177 | pub fn cache_dir() -> Option<std::path::PathBuf> {
```

Root cause: `ort-sys 2.0.0-rc.10`'s `cache_dir()` helper is gated on
`target_os` being one of `windows | linux | macos`. There is no
`target_os = "android"` arm, so the import in `build.rs:26` doesn't
resolve and the build script fails before any compilation begins.

The workspace pins ORT at `2.0.0-rc.10` because the vendored
silero / whisper / piper crates require it (see CLAUDE.md). Unblocking
requires one of:

- **(a)** Upstream PR to `pyke/ort` adding a `target_os = "android"`
  arm in `ort-sys/src/internal/dirs.rs` (the Linux arm's `XDG_CACHE_HOME`
  fallback would likely work as-is for Android).
- **(b)** Bumping ORT past rc.10 in lockstep with the speech vendor
  patches — non-trivial because three vendored crates rely on the
  current ort API surface.
- **(c)** A workspace `[patch.crates-io] ort-sys = { path = … }` that
  adds the Android arm locally. Cleanest short-term fix if anyone
  pursues this.

**Until resolved, the recommended Android setting is
`--embedder-backend none`** (BM25-only retrieval). This is a graceful
fallback, not a broken feature — the retrieval-quality benchmark suite
passes at 100% strict recall on the 91-query English benchmark under
BM25-only defaults, and `KNOWN_FAILING_QUERIES_HYBRID` is empty
post-issue-#45 so there are no documented hybrid-only wins to lose.

Follow-up GitHub issue tracking this: [#157](https://github.com/hherb/primer/issues/157).

## What works, what doesn't (as of 2026-05-27)

| Feature                                       | Status                                    |
| --------------------------------------------- | ----------------------------------------- |
| `cargo build --bin primer` (default features) | ✅ works on Termux                         |
| `cargo test --workspace` (default features)   | 🟡 runs; full pass tally not captured this session |
| Cloud REPL (`--backend cloud`)                | ✅ works; recommended Phase 0 setup        |
| On-device Ollama REPL (`--backend ollama`)    | 🟡 functional but ⚠️ **too slow for conversational use at 4B Q4 on CPU** |
| NPU REPL (`--backend qnn`, `--features qnn`)  | 🟡 wired + host-tested; device-unverified (needs QAIRT + a Genie bundle) — see benchmark section below |
| Session persistence (`~/.primer/<slug>.db`)   | ✅ works                                   |
| `--resume <uuid>` flow                        | ✅ works (mind the `--name` gotcha)        |
| Auto-seeded knowledge base (91 passages)      | ✅ works                                   |
| Engagement / extractor / comprehension chain  | ✅ wires up; full quality not bench'd here |
| Hybrid retrieval (`--embedder-backend fastembed`) | ❌ `ort-sys 2.0.0-rc.10` has no Android arm (see section above) |
| Voice mode (`--speech`)                       | ❌ not validated on Android (Phase 2 work) |
| GUI (Tauri desktop binary)                    | ❌ not in scope                            |
| Reasoning-mode models on Ollama               | ❌ leak `<think>` / `<unused>` tokens into response |

## Phase-implication note

The "phone-as-Primer viability" question raised in the [technical spec](../background_research/primer_technical_spec.md)
asks whether a current consumer phone can deliver acceptable Socratic
dialogue latency. The empirical answer from this validation:

- **CPU-only on Snapdragon 8 Elite:** no — even 4B Q4 is too slow.
- **NPU-accelerated (QNN, Phase 1.2):** untested but the entire point
  of the Hexagon NPU is to make this work.
- **Cloud backend on the phone over WiFi:** yes — fully usable today.

This sharpens the priority of ROADMAP Phase 1.2 (`QnnBackend`) relative
to Phase 1.1 (`LlamaCppBackend`): a CPU-targeted llama.cpp path on
Snapdragon CPU will not be conversational either. The Phase 1.1 work
is still valuable as a portable fallback, but the standalone-phone
product story flows through Phase 1.2.

## Run the QNN benchmark (Phase 1.2)

The `qnn_bench` example is the **device throughput + thermal test** for the
NPU backend. It loops a corpus of Socratic dialogue-continuation prompts
for a fixed wall-clock window, measures time-to-first-token and
steady-state decode rate per prompt, samples `/sys/class/thermal` every two
seconds, and prints a pass/fail verdict against the Phase 1.2 acceptance
targets: **sustained decode ≥ 15 tok/s, TTFT < 3 s, peak ≤ 70 °C**.

Prereqs: a QAIRT SDK install (`libGenie.so` + dependencies) and a
pre-compiled Genie bundle (e.g. Qwen3-4B from AI Hub). The QAIRT install
and bundle-fetch steps are in the [QNN validation runbook](qnn-validation-runbook.md).

```bash
cd ~/primer/src
~/.cargo/bin/cargo run --release --example qnn_bench --features qnn -- \
    --bundle-dir ~/primer-bundles/qwen3-4b \
    --duration-secs 900 \
    --thermal-out ~/storage/shared/primer-thermal.csv
```

Flags: `--bundle-dir` (required), `--qairt-lib-dir` (defaults to
`<bundle>/../qairt/lib/aarch64-android/`), `--prompts` (defaults to the
shipped `data/bench/socratic_prompts.jsonl`), `--duration-secs`,
`--thermal-out` (CSV path; omit to skip), and `--min-decode-tps` /
`--max-ttft-ms` / `--max-peak-temp-c` to relax the targets when probing a
smaller model. The process **exits non-zero if any target fails**, so it
can gate a device-side script. `/tmp` is not writable under Termux — write
the CSV under `~/storage/shared/` or `$TMPDIR`.

The pure aggregation logic (percentiles, pass/fail, thermal parsing) is
host-tested via `cargo test -p primer-inference --features qnn`; only the
backend round-trip and the sysfs reads are device-only.
