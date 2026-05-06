# Handoff — 2026-05-06 09:34 +0800 — Doc-correctness sweep

**Branch:** `main` (no feature branch needed; doc-only)
**Test status:** 548 passed, 0 failed (workspace).

## What this session did

Corrected stale claims about the seed corpus across four files. The previous brief and several in-tree docs still claimed the seed corpus was "55 passages in the 'space' cluster" with "more clusters to follow", but in reality:

- The corpus has 55 passages spanning **all five planned clusters**: space (11), body (12), how-things-work (11), life (10), earth/weather (11).
- `primer-kb-load/tests/retrieval_quality.rs` already runs **50+ canonical child queries across every cluster** and asserts top-5 results contain expected terms.
- All of this shipped in PR #34 (`f9ee28a`) and `101a5d3`. The ROADMAP and README hadn't been updated to reflect it.

## Files touched

| File | What changed |
|---|---|
| `data/seed/seed_passages.en.jsonl` | Header comment block (`#`-prefixed lines) — no longer says "more clusters to follow"; now enumerates all five clusters that ship in this file |
| `README.md` | KB bootstrapping bullet (line ~46) corrected; Phase 0 status paragraph corrected |
| `ROADMAP.md` | Phase 0.2 seed-corpus line is now `✅` and describes all five clusters; the previously-unchecked `[ ]` cluster-expansion bullet was removed; Phase 0 exit-criteria paragraph corrected |
| `CLAUDE.md` | Project-shape paragraph upgraded to "Phase 0.1 + 0.3 done; 0.2 partially done" with explicit enumeration of remaining items (Wikipedia ingestion + retrieval-params tuning) |

## What this session did NOT do

- No code changes.
- No new tests; the existing `retrieval_quality.rs` already covers all five clusters with 50+ queries (verified by re-running `cargo test -p primer-kb-load` post-edit; both integration tests pass).
- No commits (per the `/nextsession` brief: `Only create commits when requested by the user`). The four changes are unstaged and ready for `git add` + commit if Horst wants.
- No new NEXT_SESSION.md branch direction beyond what's now in the repo.

## Why I checked rather than charging into Phase 0.2 work

Horst pointed out that he thought option A (corpus expansion) was already done in `claude/plan-kb-bootstrap-2b0Xm` — and he was right. PR #34 (whose source branch was `claude/plan-kb-bootstrap-2b0Xm`) shipped the cluster expansion, but the in-tree docs hadn't caught up. Closing this gap before starting genuinely new work prevents future sessions from re-doing the same expansion.

## What's next

Two genuine open items remain in Phase 0.2:

1. **Simple English Wikipedia ingestion script** (Python, under `data/ingest/`). License: CC-BY-SA-3.0. Multi-session work; needs decisions on article selection (curated portals only, given children's-product safety constraints), chunking strategy, and storage size.
2. **Tune `RetrievalParams` / `HybridParams` defaults** against the broader corpus. Gated on (1).

After Phase 0.2 closes, Phase 1 (local llama.cpp inference) is the natural next milestone, but that's planned to be Bernd Brinkmann's work and is genuinely waiting on him.

## Carried-forward open items

(Unchanged from the previous handoff; preserved here for continuity.)

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

## Resume commands

```bash
cd /Users/hherb/src/primer
git status                       # should show 4 unstaged doc changes if not yet committed
cd src
~/.cargo/bin/cargo test --workspace
# Expected: 548 passed, 0 failed.
```

## Patterns honoured

- No code changes means no test impact, but the doc-only edits to a file the loader actually reads (`data/seed/seed_passages.en.jsonl`) were verified by re-running the integration test that actually loads it. **Verify before claiming complete** even for "trivial" doc edits.
