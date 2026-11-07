---
name: finalcheck
description: final pre-merge pass on the current PR (or the earliest open PR when the branch has none) — review, fix everything, then drive GitHub CI to green
disable-model-invocation: false
allowed-tools: Bash(git *), Bash(gh *), Bash(cargo *), Bash(~/.cargo/bin/cargo *), Bash(uv *), Bash(bash *), Bash(actionlint *)
---
Run these steps in order on the PR for the current branch. First run
`pwd && git branch --show-current && git worktree list`. If the PR's branch is checked out in a worktree,
`cd` into it. Then find the PR with `gh pr view --json number,url,headRefOid`.

If the current branch has no PR, take the earliest open one instead:
`gh pr list --state open --json number,headRefName,createdAt --jq 'sort_by(.createdAt) | .[0]'`.
If there are no open PRs at all, stop and tell me. Otherwise say which PR you picked, then switch to it:
- If its branch is checked out in a worktree (`git worktree list`), `cd` into that worktree.
- Otherwise run `gh pr checkout <number>`. Do this only when `git status --porcelain` is empty. With
  uncommitted changes, stop and tell me rather than stashing or discarding them.

Then re-run `gh pr view --json number,url,headRefOid` to confirm you are on the PR.

Every cargo command runs from `src/` (the workspace root) as `~/.cargo/bin/cargo`, never a Homebrew cargo
(see CLAUDE.md). A foreground `cargo test --workspace` can exceed the 600 s tool timeout, so run it with
`run_in_background` and grep the log.

1. **Review.** Invoke the `pr-review-toolkit:review-pr` skill via the Skill tool and let it finish.
2. **Fix.**
   - First bring the branch up to date: `git fetch origin && git merge origin/main`, and resolve any
     conflicts. The PR squash-merges, so a merge commit on the branch is fine.
   - Then invoke the `fixall` skill via the Skill tool on that review's findings. Per CLAUDE.md's
     "fix it or file it", every confirmed finding is either fixed in this PR or lodged as a GitHub issue
     with file refs and a failure scenario. None may live only in the PR body or the chat.
   - `fixall` ends by committing and pushing into the PR. Before moving on, confirm the push landed:
     `git fetch origin && git status -sb` must show no `ahead`, and the PR's `headRefOid` must equal
     `git rev-parse HEAD`.
   - If the fixes change what `NEXT_SESSION.md` says, update it and then re-copy it over this session's
     handoff file in `docs/handoffs/`. The handoff is an exact copy, never separately edited:
     `cmp NEXT_SESSION.md docs/handoffs/<ts>.md` must print nothing.
3. **Drive CI to green.** Loop:
   - Watch the checks for the pushed head commit with `gh pr checks --watch --fail-fast`. Checks can take a
     minute to register, so wait if it reports none yet.
   - `ci.yml`'s `changes` job filters out `**/*.md`, `docs/**`, `.gitignore` and `LICENSE`, matched against
     the **whole PR diff**. A PR that is docs-only overall shows `detect code changes` passing and every
     other job **skipped**. Skipped counts as passing for the required check (issue #404), so that is
     green. A docs-only push onto a PR that also changes code still runs everything.
   - `ci.yml` uses `concurrency: cancel-in-progress`, so a run on an older commit shows CANCELLED once a
     newer push lands. That is normal. Only the head commit's run counts.
   - All green → done. Report the PR URL and the final check list. "All green" means every check on the
     head commit passed. That includes the drift guards (`cargo check (non-default features)`,
     `cargo clippy (macOS feature combos)`, `cargo build (aarch64-linux-android)`,
     `validate gradle wrapper`) and `detect code changes`, not just the required ones. A heavy job
     showing **skipped** on a PR that changes code means the `changes` filter misfired: report it. To see the required contexts, run
     `gh api repos/hherb/primer/branches/main/protection --jq .required_status_checks.contexts`. Repo
     rulesets are listed by `gh api repos/hherb/primer/rules/branches/main`.
   - A check failed → read the cause: `gh run list --branch "$(git branch --show-current)" --limit 5`, then
     `gh run view <run-id> --log-failed`. Reproduce it locally with the command the failing step runs
     (`.github/workflows/ci.yml` is the source of truth; these mirror it). Run from `src/`:
     - **`cargo test (default features)`:**
       - `cargo fmt --all -- --check`
       - `cargo clippy --workspace --all-targets -- -D warnings`
       - `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items`
       - `RUSTFLAGS="-D warnings" cargo test --workspace --no-fail-fast`. The `RUSTFLAGS` matters: CI fails
         on test-only warnings that a plain local `cargo test` lets through.
     - **`cargo check (non-default features)`:** run the failing step's own command. They are:
       - `cargo check`/`clippy -p primer-speech --features supertonic`
       - `cargo check -p primer-embedding --features fastembed`
       - `cargo clippy -p primer-inference --features llamacpp`
       - `cargo clippy -p primer-kb-load --features fastembed`
       - `cargo test -p primer-engine --features ollama-embedding,openai-compat-embedding`
       - `cargo clippy -p primer-cli --features speech`
       - `cargo test -p primer-gui --features speech --lib commands::voice`
       - `cargo test -p primer-gui --no-default-features --features android-native --lib commands::voice`
       - `cargo test -p primer-speech --features voice-loop,android-native --lib android`
       - the rustdoc feature-arm loop (the `cargo doc (non-default feature arms)` step: the same
         `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items -p … --features …`
         list).

       Clippy steps take `--all-targets -- -D warnings`.
     - **`cargo clippy (macOS feature combos)`:** runs natively on this Mac. Use the step's command, e.g.
       `cargo clippy -p primer-cli --features speech,macos-native --all-targets -- -D warnings`.
     - **`cargo build (aarch64-linux-android)`:**
       - Run `../scripts/check-ort-sys-android-cfg.sh` first.
       - `rustup target add aarch64-linux-android --toolchain 1.88`.
       - Export the four NDK vars the "Export NDK toolchain env" step writes (`CC_`/`CXX_`/`AR_aarch64_linux_android`,
         `CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER`, all API 24). On macOS, point them at the local NDK's
         `toolchains/llvm/prebuilt/darwin-x86_64/bin`.
       - Then run the failing `cargo build --target aarch64-linux-android …` line verbatim. Keep its
         `--no-default-features`: Android stays BM25-only (#157).
     - **`validate gradle wrapper`:** this is a supply-chain checksum. Never regenerate or replace the
       wrapper jar to make it pass. Stop and report.
     - **A workflow file changed:** `actionlint`.

     Judge each command by its **exit code**, never by filtered output: `cmd | tail` reports `tail`'s
     status. Fix the root cause, using TDD where it is a code bug. Confirm the fix passes locally, commit
     with a message naming the CI failure, push, and loop again.
   - **Linux-only failures** (CI is `ubuntu-latest` and macOS passes locally): reproduce in Docker
     (`rust:1.88-bookworm`, plus the apt packages from the "Install Tauri Linux system deps" step; start
     the daemon with `open -a Docker`). Don't guess.
   - **A head-commit job that is CANCELLED or timed out with no log** was not superseded. The runner was
     killed or hit `timeout-minutes` (the macOS job caps at 30). Treat it as a code bug, such as a hung
     test or a runaway build, and find the cause. Don't just rerun it as a flake.
   - If the failure is not caused by the code (runner/infra outage, network flake, a cdn.pyke.io
     ort-runtime download failure, or a missing secret), rerun once with `gh run rerun <run-id> --failed`.
     If it fails the same way again, stop and report rather than changing code to work around it. The
     Actions API returns empty or stale data during an outage, so re-query before concluding a run never
     happened.
   - Never "fix" CI by skipping, `#[ignore]`-ing or deleting tests, or by adding `#[allow(...)]`. Don't
     add entries to the `KNOWN_FAILING_QUERIES*` / `KNOWN_LEAKING_OFF_TOPIC*` lists either, or loosen a
     recall floor or other test threshold. And don't weaken lint/CI config or the vendored-patch guards.
     Each needs my explicit permission. If the same failure survives three fix attempts, stop and report
     what you tried.
4. **Not covered by CI.** If the PR touches these, run the check locally and include the result in the
   report:
   - **Retrieval**, i.e. `primer-knowledge`, `primer-kb-load`, `primer-embedding`, retrieval consts, or
     `data/seed/`: `cargo test -p primer-kb-load --features fastembed --test off_topic_gate --test
     retrieval_quality_hybrid --test retrieval_quality_hybrid_de`. It downloads BGE-M3 (~570 MB) on the
     first run.
   - **`macos-native-26` code:** `cargo clippy -p primer-speech --features voice-loop,silero,cpal,macos-native-26
     --all-targets -- -D warnings`, plus the same `cargo doc` arm. It needs the macOS 26 SDK.
   - **`data/ingest/`:** `cd data/ingest && uv run pytest` (uv only, never pip).
   - **`primer-gui/ui/*.js`:** there is no JS runner, so only the `include_str!` contract tests
     (`primer-gui/src/*_contract.rs`) cover it. Say so in the report.
   - **Device- or hardware-gated paths** (QNN/Genie, the Android JNI bridge, llama.cpp benches, real
     audio): list them as not verified rather than claiming them.
