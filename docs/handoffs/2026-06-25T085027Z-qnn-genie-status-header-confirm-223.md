# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-25. Branch `qnn-genie-status-header-confirm-223` (committed locally at `15b3134`, **not yet pushed** — owner-authorised PR is item #0). `main` is at `c9d572c`.

**The previous handoff is fully discharged:** its headline (push the `supertonic-openrail-license-assessment-170` branch + open a PR) was already done before this session started, merging as **PR #263 (`c9d572c`)**. So this session took the host-completable slice the prior handoff explicitly flagged: **#223 — confirm the Genie context-limit status value `4` against the authoritative QAIRT header via web search** (rather than waiting on the SDK login).

## What we shipped this session (branch `qnn-genie-status-header-confirm-223`)

- **`15b3134` — qnn: confirm context-limit status 4 vs authoritative GenieCommon.h (#223).** Host-only docs + rename; **behaviour unchanged, still value-`4`-pinned**. The graceful-completion path keyed off status `4` was reverse-engineered from on-device `genie.log`; it is now confirmed against the real header.
  - **Source of truth located by web research (no Qualcomm login needed):** public copies of QAIRT `GenieCommon.h` in the **official `qualcomm/qidk`** and **`qdsp6sw/qualcomm-ai-engine-direct-sdk`** GitHub org repos, cross-checked against two independent vendored copies (`ChinmayShringi/graft-analysis`, `DreamFekk/YoloTouchHelp`) and Qualcomm's published docs page. All agree:
    ```c
    #define GENIE_STATUS_WARNING_CONTEXT_EXCEEDED  4
    ```
  - **Findings vs the issue's assumptions:**
    1. **Value `4` is correct** — the graceful path needed **no** re-pointing. The reverse-engineered value held.
    2. **Canonical name differs:** `GENIE_STATUS_WARNING_CONTEXT_EXCEEDED`, not the reverse-engineered `GENIE_STATUS_CONTEXT_LIMIT_EXCEEDED`. It is a positive **warning** code (1 aborted / 2 bound-handle / 3 paused / 4 context-exceeded; 0 success; negatives are errors), not an error.
    3. **It lives in `GenieCommon.h`**, not `GenieDialog.h` as the issue title assumed. It's a `#define` over `typedef int32_t Genie_Status_t`, not a C `enum`.
  - **Code/doc changes (4 files):**
    - `src/crates/primer-qnn-sys/src/bindings.rs` — renamed the const to `GENIE_STATUS_WARNING_CONTEXT_EXCEEDED`, rewrote its doc to cite the header (keeping the on-device diagnosis as corroboration), corrected the `Genie_Status_t` type doc (typedef + #defines, positive = warnings).
    - `src/crates/primer-inference/src/qnn/genie/mod.rs` — updated the import, rustdoc link, `classify_query_status` match arm, and the two test usages; improved the "other non-success statuses are errors" test comment to name the sibling warnings (1/2/3) we deliberately keep treating as errors (only `4` is graceful).
    - `docs/devel/03-inference-and-pedagogy.md` — QNN `FinishReason::Length` producer now cites `GENIE_STATUS_WARNING_CONTEXT_EXCEEDED` (`= 4` in `GenieCommon.h`).
    - `ROADMAP.md` — new completed bullet for #223; fixed the live symbol-name reference on the network-backend recovery bullet.

**What this session deliberately did NOT do:** change any behaviour. The graceful-completion semantics are byte-identical; only the symbol name + provenance docs changed. No on-device re-validation is needed (value `4` confirmed unchanged).

### Host gates — green
- `~/.cargo/bin/cargo test` (default features): **all pass, exit 0** (no FAILED/panicked).
- `~/.cargo/bin/cargo test -p primer-inference --features qnn qnn::genie`: 30 pass.
- `~/.cargo/bin/cargo test -p primer-qnn-sys`: 6 pass.
- `~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `~/.cargo/bin/cargo fmt -p primer-qnn-sys -p primer-inference -- --check`: clean.
- Intra-doc link `[GENIE_STATUS_WARNING_CONTEXT_EXCEEDED]` resolves (absent from `cargo doc` diagnostics; the doc warnings that do fire are all pre-existing private-item links unrelated to this change, and `cargo doc -D warnings` is not a CI gate).
- README **not** updated by design — it carries no reference to this const and the user-facing QNN status is unchanged.

## What's next (concrete acceptance criteria)

### 0. ⭐ Push + open the PR (owner-authorised, outward-facing)
- `cd /Users/hherb/src/primer && git push -u origin qnn-genie-status-header-confirm-223 && gh pr create`. **Closes #223.** Host-only docs + rename; safe.

### 1. Other actionable host-side backlog (no device)
- **#192** — manual smoke: macOS-native STT + injected non-AVSpeech TTS (Piper/Supertonic) audio path. Needs a mic + macOS build.
- **#170 Stage E** — in-loop A/B TTFA/RTF numbers for Supertonic vs Piper/macOS-native *inside the voice loop* (the A.5 spike measured the synth primitive in isolation). Needs a mic + audio bench. Overlaps #192.

### Carried / owner-or-hardware-gated (unchanged)
- **#170 Stage F — Hindi preview→stable:** gated on, in order: (a) native-speaker review of `prompts/hi.toml` (grep `# REVIEW:`); (b) a Hindi corpus (none exists — NCERT / Pratham StoryWeaver / Wikisource candidates, each needs a per-source licence check); (c) `tests/common/hi.rs` benchmark queries + retrieval-quality/sweep tests mirroring the EN/DE shape; (d) real-LLM smoke. The **licence** sub-gate (OpenRAIL-M weights) is done (PR #263). **Promotion changes when those clear (one commit):** add `Self::Hindi` to `Locale::ALL` (`primer-core/src/i18n.rs:59`); `status = "stable"` in `hi.toml:33`; update `locale_all_excludes_hindi_until_translation_reviewed` (`i18n.rs:387`, 2→3); remove/invert `list_locales_excludes_preview_hindi` (`primer-gui/src/commands/settings.rs:116`); flip the Hindi README header PREVIEW→STABLE. **Clause (e) of the OpenRAIL-M licence — an express "AI-generated voice" disclosure — must ship before any default Supertonic flip** (cheap onboarding/voice-mode one-liner; note it on whatever issue tracks the Supertonic default flip).
- **#166 item #1** — owner-run the WhisperStream reuse-invariance smoke on a real model + two 16 kHz mono WAVs (commands at the bottom). Owner-gated.
- **#260 / #259** — Android-voice on-device acceptance: 10 consecutive clean voice turns, sustained ~10-min session, mid-session airplane toggle, no-barge-in. Needs the RedMagic 11 Pro + mic.
- QNN stable-token-across-reboots gate; NPU pedagogy/answer-quality tuning; latency-routing calibration; #135 glib (blocked on Tauri 3).

## Open decisions / risks

- **Branch not yet pushed at handoff** (item #0) — outward-facing, owner-authorised.
- **The header value is confirmed from *public mirrors*, not a freshly-downloaded QAIRT SDK.** The mirrors are highly consistent (two Qualcomm-org repos + two independent vendored copies + the official docs page all show `4`), and the value matches the prior on-device diagnosis, so confidence is high. If the owner ever has the gated SDK in hand, a 10-second `grep GENIE_STATUS_WARNING_CONTEXT_EXCEEDED include/Genie/GenieCommon.h` is the final belt-and-braces check — but it is not blocking.
- **No behavioural change shipped**, so no on-device re-test is implied by this commit. The `4`→graceful mapping is unchanged.

## Patterns to reuse, not reinvent

- **Gated-SDK header constants can often be confirmed from public GitHub mirrors.** When a value is "reverse-engineered, needs the vendor header," try GitHub *code search* (`gh api -X GET search/code -f q='SYMBOL'`) before assuming it's login-gated — Android sample apps and SDK-helper repos routinely vendor the headers. Prefer official-org copies (`qualcomm/*`, `quic/*`) and cross-check ≥2 independent copies + vendor docs before trusting a value.
- **A `-sys` crate's constants should carry the exact upstream header names.** When a reverse-engineered name diverges from the confirmed header, rename to match — it removes the divergence permanently and lets anyone cross-reference the SDK. Keep domain names (e.g. our `QueryOutcome::ContextLimit`) separate from the FFI-mirror names.
- **Qualcomm docs.qualcomm.com pages are JS-rendered SPA shells** — `WebFetch` returns an empty "Qualcomm Documentation" header. Don't waste calls on them; go straight to GitHub code search for the actual header bytes.
- **The host-actionable backlog is thin** — most open issues are owner/hardware/SDK-gated. When a session opens with the headline already merged, survey `gh issue list` + this file's "Carried" section and pick the genuinely host-completable slice (this session: #223 via web research).

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git fetch && git log --oneline -4
git checkout qnn-genie-status-header-confirm-223           # this session's branch (15b3134)
# Item #0 — push + PR: git push -u origin qnn-genie-status-header-confirm-223 && gh pr create   # closes #223

# === Standard workspace gate ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test                                       # all default-feature tests (exit 0 this session)
~/.cargo/bin/cargo test -p primer-inference --features qnn qnn::genie   # QNN status-classification tests
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo fmt --all -- --check

# === If re-confirming the header value (optional belt-and-braces) ===
gh api repos/qualcomm/qidk/contents/GenAI-Solutions/AI-Assistant/app/src/main/cpp/genie/GenieCommon.h \
  --jq '.content' | base64 -d | grep GENIE_STATUS_WARNING_CONTEXT_EXCEEDED

# === Carried: owner-run the #166 reuse smoke (needs a model + two 16 kHz mono WAVs) ===
PRIMER_WHISPER_MODEL=/path/to/ggml-small.en.bin \
PRIMER_WHISPER_AUDIO_A=/path/to/utterance_a.wav \
PRIMER_WHISPER_AUDIO_B=/path/to/utterance_b.wav \
  ~/.cargo/bin/cargo test -p primer-speech --features whisper \
  --test whisper_stream_reuse -- --ignored --nocapture
```

## Reporting back

- #223 is **resolved host-side**: status `4` confirmed against the authoritative QAIRT `GenieCommon.h` (`GENIE_STATUS_WARNING_CONTEXT_EXCEEDED = 4`) via public org-repo mirrors + docs; const renamed to the canonical name. Value unchanged → no on-device re-test needed. Commit `15b3134`; push + PR is owner-authorised item #0.
- No behaviour changed; all host gates green.
- The GUI is a full app, not a scaffold.
