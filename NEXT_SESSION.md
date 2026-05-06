# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-06 (after a doc-correctness sweep on `main` correcting stale claims about the seed corpus).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **548 tests** across the workspace as of this writing.
3. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first — Horst may have made interim changes.

## Branch status

`main`, clean working tree (after the doc-correctness sweep is committed). The previous session (per the prior NEXT_SESSION.md) shipped Phase 0.3 close on `feature/session-break-suggestion`; in the interim PR #34 (hybrid retrieval + KB ingestion infrastructure), PR #35 (the Phase 0.3 close), and the `101a5d3` doc commit have all merged to `main`.

## What we shipped this session

A documentation-correctness sweep. The previous brief and several in-tree docs still claimed the seed corpus was "55 passages in the 'space' cluster" with "more clusters to follow" — but in reality the corpus has expanded to all five planned clusters (space, body, how-things-work, life, earth/weather), the retrieval-quality test has 50+ canonical queries spanning every cluster, and that all shipped in PR #34 / commit 101a5d3.

Files updated:

- `data/seed/seed_passages.en.jsonl` — header comment now reflects the actual five-cluster coverage (no more "to follow" in the corpus that already has them)
- `README.md` — KB bootstrapping bullet (line ~46) corrected; Phase status line corrected to reflect 5-cluster coverage
- `ROADMAP.md` — Phase 0.2 line for the seed corpus is now `✅` and describes all five clusters; the previously-unchecked `[ ] Expand the seed corpus` bullet was removed (the work is done); Phase 0 exit-criteria paragraph corrected
- `CLAUDE.md` — project-shape paragraph upgraded from "Phase 0.1 + 0.3 done" to "+ Phase 0.2 partially done"; the bullet now correctly enumerates what's still ahead in Phase 0.2 (Wikipedia ingestion + retrieval-params tuning)

The seed file's header edit is a comment block (`#`-prefixed lines) which the loader already skips; the retrieval-quality integration tests were re-run and still pass.

**No code changes.** Doc-only.

## What's next — Phase 0.2 remaining work

Two genuine open items remain in Phase 0.2:

### Option A — Simple English Wikipedia ingestion script (recommended; meaty)

A standalone tool that downloads a Simple English Wikipedia dump, filters/cleans articles, splits into ~200-word passages with age tags, and emits JSONL compatible with `primer-kb-load`. Python is fine per the ROADMAP.

Concrete acceptance criteria:

1. New script under `data/ingest/` (e.g. `data/ingest/simple_wikipedia.py`).
2. Produces JSONL with the schema `primer-kb-load` already accepts: `{ "id": "...", "source": "...", "license": "...", "attribution": "...", "text": "...", "topics": [...] }`. Each passage's `id` is namespaced (e.g. `wiki-simple:en:<slug>`); `source` and `attribution` are filled correctly per CC-BY-SA; the `license` field is set to `CC-BY-SA-3.0`.
3. Article selection rule documented (e.g. "all articles in selected children's-curriculum portals" or "articles with at least N pageviews"). Multi-session work.
4. Chunking strategy documented (paragraph-based, with a soft 200-word target and never breaking mid-sentence).
5. Smoke run produces ≥500 passages and `primer-kb-load --knowledge-db /tmp/wiki.db --locale en data/ingest/output.jsonl` ingests them without errors.
6. The retrieval-quality integration test continues to pass when the Wikipedia corpus is layered on top of the hand-drafted seed corpus (canonical queries should still surface the curated passages — at minimum, the BM25 leg should not be drowned).
7. ROADMAP.md and CLAUDE.md updated.

Caveats / decisions to make in-session:

- **License compliance:** Simple English Wikipedia is CC-BY-SA-3.0 (and a contributor-attribution file is required). The script must record per-passage attribution metadata and the codebase must already preserve it through retrieval — `passages.<pack>_meta.attribution` already exists per the schema, but verify it's surfaced where it needs to be for any downstream display.
- **Privacy / safety filter:** Children's-product context. Some Wikipedia articles cover violence, sex, drugs, etc. inappropriately for the Phase 0 audience. The first cut should restrict to a curated portal list (Science, Geography, Biology, History) rather than dumping everything.
- **Storage size:** A useful Simple English Wikipedia subset is multi-GB after embedding. The default `--knowledge-db` path doesn't exist yet (CLI defaults to `:memory:`). Decide whether the Wikipedia corpus auto-loads on first run (likely no — opt-in via a flag or a separate bootstrap subcommand).

### Option B — Tune `RetrievalParams` / `HybridParams` defaults

This is gated on Option A landing — there isn't enough corpus yet to tell whether the current defaults (`top_k=3`, `bm25_top_k=20`, `vector_top_k=20`, `final_top_k=5`, `rrf_k=60`) are well-chosen. Once the Wikipedia corpus is in:

1. Add benchmark queries that span the wider corpus (e.g. children's questions where the right answer is a Wikipedia article rather than a hand-drafted seed passage).
2. Sweep `top_k`, `min_score`, and `rrf_k`. Lower `rrf_k` weighs the very top of each ranked list more; higher flattens. Industry default is 60; the spec at `docs/superpowers/specs/2026-05-05-hybrid-retrieval-design.md` calls this out.
3. Land the chosen defaults; update `primer_core::consts::retrieval` accordingly.

## Open decisions / risks

Carried-forward open items (still relevant from prior sessions):

- Smoke #6 (Ollama daemon down) deferred verification.
- `is_request()` heuristic conflates config errors with network errors.
- HTTP-date form `Retry-After` silently dropped.
- Pre-flight auth check at startup deferred.
- Voice-mode bridging hook is data-only.
- `close_session` may drop the final classifier task's `turn_classifications` row on a short conversation.
- Identifier-format divergence across structured-output crates (`llm:{model}` vs `llm:{backend}:{model}`).
- `apply_comprehension` insertion policy (only updates concepts already in `learner.concepts`).
- Concepts are monotonic on `learner_concepts` — saved-once stays saved.
- No cancellation token on streaming-task spawns.
- Background-task spawn order is load-bearing on serialized backends.
- "I just don't want to see this word" escape valve for vocab not yet implemented.

New (this session): none. The doc-correctness sweep didn't surface any code-level issues.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed standing.)

- **Pure functions in `primer-core`** for algorithmic cores — tested by injection of `now`, no async, no I/O.
- **`#[cfg(test)]` test seams with `pub` accessor methods** — zero production cost.
- **Locale-keyed templates in TOML packs** with `{placeholder}` substitution at trait method level.
- **Carrier structs with `disabled()` no-op constructors** for parameter bundles that not all callers need to configure.
- **Lookup-table seeding for new closed-enum variants** — no schema migration needed.
- **Constants in `consts.rs` submodules.** No magic numbers anywhere.
- **TDD discipline.** Tests first; watch them fail; implement to green.
- **File-size hygiene.** Keep modules under 500 lines.

## Exact commands needed to resume

```bash
# Resume on main (the doc-correctness sweep is either committed or about to be):
cd /Users/hherb/src/primer
git status                       # confirm where you are
git log --oneline -10            # recent activity

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 548 passed, 0 failed (as of 2026-05-06).

~/.cargo/bin/cargo clippy --workspace --all-targets
~/.cargo/bin/cargo fmt --all -- --check
```

For real-LLM smoke testing (Anthropic):

```bash
cd /Users/hherb/src/primer/src
RUST_LOG=debug ~/.cargo/bin/cargo run --bin primer -- \
    --backend cloud --name SmokeTester --age 9 --no-persist --verbose 2>&1 | tee /tmp/smoke.log
```

For the speech build path: `~/.cargo/bin/cargo build --workspace --features primer-cli/speech`.

For the embedding feature build path:
```bash
~/.cargo/bin/cargo build --workspace --features primer-cli/embedding
~/.cargo/bin/cargo run --bin primer -- --embedder-backend fastembed ...
# First run downloads BGE-M3 (~570 MB) into ~/.cache/primer/models/.
```

For inspecting the seed corpus + running the retrieval-quality test:
```bash
~/.cargo/bin/cargo test -p primer-kb-load
# 7 unit tests + 2 integration tests should pass; the canonical-queries
# test exercises 50+ queries across all five clusters.
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- If you discover that Horst did interim work that changes the plan, flag it.
