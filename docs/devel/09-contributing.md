# Contributing

Welcome. The Primer is a young open-source project — a Socratic AI learning companion for children — and outside contributions are genuinely wanted. This chapter is for the contributor who has read the previous eight chapters (or at least [chapter 1](01-getting-started.md) and [chapter 2](02-architecture-overview.md)), has a change in mind, and wants to know how to land it. It covers the repo workflow, the commit and code-style rules CI enforces, two non-negotiable codebase conventions that deserve special attention because reviewers will flag them every time, where to find work that needs doing, and what to expect when you open a pull request.

The tone of the project is collaborative and direct. Reviewers will ask for changes; this is normal and not personal. The goal is shared — a codebase a child can rely on — and any PR that moves the project toward that goal is welcome, no matter how small.

## Repo workflow

The standard fork-and-PR workflow applies. In brief:

1. Fork the repo on GitHub.
2. Clone your fork and add the upstream remote: `git remote add upstream https://github.com/<upstream-owner>/primer.git`.
3. Branch from `main`. Use a short, descriptive name with a category prefix:
   - `feature/...` for new functionality (e.g. `feature/qnn-backend`)
   - `fix/...` for bug fixes (e.g. `fix/resampler-tail-flush`)
   - `docs/...` for documentation-only changes (e.g. `docs/devel-manual`)
   - `refactor/...`, `test/...`, `chore/...` for the obvious cases
4. Keep PRs focused. One logical change per PR is much easier to review than a sweep of unrelated improvements.
5. Push to your fork and open the PR against `main` upstream.

Rebase on top of `main` before opening the PR, and again if `main` advances during review. Merge commits in feature branches make `git log` noisy.

> **Note:** the `src/` directory is the cargo workspace root, but it is not a git submodule — every change still goes through one PR against the repo root. See [chapter 1](01-getting-started.md) for why the workspace lives one level down.

## Commit conventions

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/). The shape is `type(scope): subject`, where the type is one of the following — these are the ones in active use in `git log`:

- `feat:` — new user-visible feature
- `fix:` — bug fix
- `docs:` — documentation only
- `refactor:` — internal restructure with no behaviour change
- `test:` — adding or fixing tests, no production change
- `chore:` — build, tooling, deps

Scope is optional but encouraged — `feat(speech):` or `docs(devel):` reads better than a bare `feat:` in `git log --oneline`. The subject should be imperative mood, lowercase, no trailing period: `add silero VAD wrapper`, not `Added silero VAD wrapper.`.

> **Note:** commits produced with AI assistance (Claude Code) carry a `Co-Authored-By:` trailer crediting the model, matching the convention already visible in `git log` and the "Claude" row in [ROADMAP.md](../../ROADMAP.md)'s contributor table. Keep the trailer on the final commit line so attribution is greppable.

> **Gotcha:** never use `--no-verify` to skip pre-commit hooks. Hooks exist for a reason; if one fails, fix the underlying issue instead of bypassing it. Likewise, never `git commit --amend` after pushing a branch — force-pushing rewrites history out from under reviewers and breaks line-comment threads. If you need to fix a pushed commit, add a follow-up commit; squashing happens at merge time.

## Code style

CI is the source of truth for style. Two commands gate every PR, both run from `src/`:

```bash
cd src
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The CI workflow at [.github/workflows/ci.yml](../../.github/workflows/ci.yml) runs `cargo fmt --check`, `cargo clippy --workspace --all-targets` with `RUSTFLAGS=-D warnings`, and `cargo test --workspace --no-fail-fast`, plus drift guards for the non-default features and an `aarch64-linux-android` cross-compile. A PR that fails any of these will not merge until it does. Run them locally before pushing — turnaround is faster on your machine than in GitHub Actions.

The toolchain is pinned to **1.88** in [rust-toolchain.toml](../../src/rust-toolchain.toml), honoured only by the rustup proxy binaries — invoke cargo as `~/.cargo/bin/cargo` so a Homebrew `cargo` on `$PATH` doesn't silently shadow it (see [chapter 8](08-testing-and-debugging.md)).

### The pre-commit hook

A version-controlled pre-commit hook lives at [.githooks/pre-commit](../../.githooks/pre-commit). On any commit that touches `.rs` files it runs `cargo fmt --all -- --check` from `src/` (and fast-skips otherwise), so a formatting drift fails locally instead of after a CI round-trip. Opt in once per clone:

```bash
git config core.hooksPath .githooks
```

The CI step is the source of truth; the hook is an early-warning copy. It skips with a one-line warning when `cargo` isn't on `PATH` and `$CARGO` is unset, so docs-only contributors aren't forced to install rustup.

### Branch protection

Branch protection on `main` is the recommended structural fix and the merge-boundary backstop: the repo settings require the `cargo test (default features)` status check to pass before a PR can merge to `main`. The local hook complements this but does not replace it — the hook closes the loop for direct `git commit`; branch protection closes it at the merge boundary regardless of contributor discipline.

Before opening a PR:

```bash
cd src
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
cargo test
```

`cargo fmt` writes the changes; `cargo fmt --all -- --check` (what CI runs) only verifies. The `-D warnings` flag promotes every clippy lint to a hard error, matching CI's `RUSTFLAGS=-D warnings` setting. If clippy flags something you genuinely cannot fix, prefer a narrowly-scoped `#[allow(clippy::lint_name)]` with a comment explaining why, over a workspace-level allow.

> **Note:** the `embedding` feature is now in `default` and built by CI (the cdn.pyke.io ORT download is proven on Linux + macOS). The `speech`, `llamacpp`, `qnn`, and the Apple-native (`macos-native` / `macos-native-26`) features pull heavy native deps (ONNX Runtime, espeak-ng, llama.cpp, the QAIRT SDK, a Swift sidecar) and are *not* part of the default `cargo test` job — CI covers them with clippy/cross-compile drift guards rather than a full test run. If your change touches `primer-speech`, `primer-inference` local backends, or `primer-embedding`, build and test the relevant features locally — see [chapter 7](07-speech-and-voice-loop.md), [chapter 8](08-testing-and-debugging.md), and [chapter 4](04-knowledge-and-retrieval.md).

## No magic numbers

Every numeric in this codebase has a home. Reviewers will flag inline literals every time, so save yourself a round-trip and place them correctly the first time:

- **Invariant constants** go in [primer-core::consts](../../src/crates/primer-core/src/consts.rs), grouped by subsystem (`retry`, `retrieval`, `classifier`, etc.). These are values that have a single right answer for the whole codebase — the BM25 `K1` parameter, the FTS5 minimum-score floor, the RRF fusion `k=60`.
- **Per-subsystem tunables** go into a `*Settings` struct (`ClassifierSettings`, `ExtractorSettings`, `ComprehensionSettings`, `HybridParams`). Every numeric field on a `*Settings` struct is backed by a default in `consts.rs` — the struct is the runtime knob; the const is the source of truth for the default.
- **Test-only literals** can stay inline; tests are pinning specific scenarios, not establishing project-wide invariants.

A drift test like `default_mirrors_consts` (see [HybridParams in primer-core](../../src/crates/primer-core/src/lib.rs)) pins the `*Settings::default()` impls to their `consts.rs` values, so changing one without the other fails the test.

> **Why:** numerics that drift in two places silently degrade behaviour. Centralising them in `consts.rs` makes the next person tuning the system able to find every related value in one place — and makes the next sweep test (see [chapter 8](08-testing-and-debugging.md)) easy to write.

## Categorical text columns are normalised

Every categorical text column in every SQLite schema in this codebase is stored as an integer foreign key into a lookup table — never as inline text. This applies to `speakers`, `pedagogical_intents`, `understanding_depths`, `concepts`, `embedding_models`, and any future categorical column. For closed Rust enums (`Speaker`, `PedagogicalIntent`, `UnderstandingDepth`), the Rust enum is the single source of truth and the lookup table is a derived projection that the storage layer regenerates and validates on every `open()`. Drift — a known id with a different name, or an unknown id — is a hard error.

The full mechanics, including how to add a new variant or a new categorical column without writing a custom migration, are in [chapter 5](05-storage-and-sessions.md). When in doubt: enum first, lookup-table seeding follows automatically.

> **Why:** unbounded text in categorical columns is how schemas drift over months. The hard-error discipline catches drift the first time it happens, not the tenth.

## Where to find open work

Several entry points, in rough order of curation:

- **[ROADMAP.md](../../ROADMAP.md)** — the phase plan. Tells you what is shipped, what is in flight, and what is next. The "what is next" section is the place to look for substantial features that match the project's direction.
- **GitHub Issues** — bugs and feature requests, with labels for `good first issue` and `help wanted` when appropriate. If an issue is unassigned and you want to take it, comment to claim it before starting work.
- **`// TODO` markers in source** — small, well-scoped tasks left by previous contributors. `git grep -n TODO src/crates` lists them. Each TODO usually has enough context inline to act on; if it does not, ask in the issue tracker before guessing.
- **[SPECULATIONS_AND_IDEAS.md](../../SPECULATIONS_AND_IDEAS.md)** — open-ended ideas, possible directions, things worth exploring but not yet specced. A good place to find work if you want to propose something the maintainers have not committed to. Open an issue first to discuss before writing code against an idea here — these are exploration prompts, not commitments.

For larger changes, open an issue or a draft PR first to discuss the approach. A 200-line PR that needs to be rewritten is more painful than a 20-line design sketch that can be redirected.

## How to ask for review

Open the PR when CI is green locally. The PR description should answer three questions:

1. **What** does this change do? One paragraph, plain language.
2. **Why** is the change needed? Link to the issue, the spec, or the user-visible symptom.
3. **How** to verify? The exact commands a reviewer can run to see the change work — usually `cargo test -p <crate>` plus, for behaviour changes, a CLI invocation.

What reviewers will check:

- **Clippy clean and tests added.** Behaviour changes need test coverage; bug fixes need a regression test that fails without the fix.
- **`decide_intent` characterization tests.** If your change touches intent routing, every existing test in the [decide_intent suite](../../src/crates/primer-pedagogy/src/prompt_builder/tests.rs) (the `prompt_builder::tests` sibling module) must still pass, and any new branch in the heuristic needs a new characterization test pinning its behaviour. See [chapter 3](03-inference-and-pedagogy.md) for why this suite is load-bearing.
- **CLAUDE.md updated if conventions changed.** [CLAUDE.md](../../CLAUDE.md) is the agent-facing source of truth for codebase conventions and gotchas. If your change adds, removes, or modifies a convention — a new schema version, a new background-task pattern, a new `*Settings` field, a new gotcha worth warning the next contributor about — update CLAUDE.md in the same PR.
- **Schema migrations are idempotent and additive.** If you added a new schema version, the migration must follow the pattern in [chapter 5](05-storage-and-sessions.md) — `CREATE IF NOT EXISTS`, `pragma_table_info` checks before `ALTER`, the whole body wrapped in `conn.unchecked_transaction()`. Review will be strict here.
- **No `unwrap()` or `expect()` in production code paths.** Test code is fine; production code propagates via `?` or maps to a `PrimerError` variant.

> **Note:** the project does not currently ship a PR template under `.github/`. Use the three-question structure above and you will be in the right shape.

Reviewers aim to leave first feedback within a few days. If a PR sits for longer than a week with no comment, ping it — sometimes things slip.

## License

The Primer is licensed under the **GNU Affero General Public License v3.0** — see [LICENSE](../../LICENSE) for the full text. The AGPL was chosen deliberately: it ensures that downstream users of the Primer (including those running it as a hosted service) retain the same right to inspect, modify, and rebuild the system that the original users have. For a children's product where the data flow includes deeply personal conversational records, that transparency is the point.

Contributions are accepted under the same license — by opening a PR you agree your contribution is licensed under AGPL-3.0. The project does not currently require a separate Contributor License Agreement.
