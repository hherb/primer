# Design: the "how do you know?" principle

**Date:** 2026-06-27
**Status:** approved

## Motivation

> A child who is told she is wrong learns to defend. A child who is asked
> how she knows learns to look again.

This epistemic stance — never confirm or correct a child's assertion
outright; instead ask how she knows or how she could check — is a core
Socratic principle that is currently *implicit* in the Primer's prompt but
not stated and not mechanically reinforced. The Primer has no signal for a
factually wrong answer (`UnderstandingDepth` is Bloom's taxonomy;
`EngagementState` is emotional), and `decide_intent` has no route for the
"the child just made a claim" moment — a substantive declarative answer
that is not yet understood falls through to the bare `SocraticQuestion`
default.

This design bakes the principle in two ways: as explicit prompt text the
LLM internalises, and as a new deterministic `PedagogicalIntent` that fires
when the child asserts a claim.

## Scope

Two layers:

1. **Prompt text** — the principle stated in the system prompt's core-
   principles list (all three locale packs) and in the CLAUDE.md
   pedagogical-principles section.
2. **A new intent** `ProbeReasoning` with `decide_intent` routing, so a
   detected substantive-but-not-yet-understood assertion deterministically
   produces a "how do you know?" stance rather than relying solely on LLM
   discretion.

Out of scope: a correctness/misconception detector (no such signal exists;
the trigger is a heuristic over the last child turn, see below).

## Layer 1 — prompt text

### System-prompt core principles (`[system_prompt].base`)

Add one bullet to the `Your core principles:` list in each pack. English
reference draft:

> - When {name} states something — right *or* wrong — do not confirm it or
>   correct it outright. Ask how they know, or how they could check. A child
>   who is told she is wrong learns to defend; a child who is asked how she
>   knows learns to look again.

`de.toml` and `hi.toml` get natural-register equivalents (not mechanical
translations), per the translator note at the top of each pack. The bullet
uses the `{name}` placeholder, which is already in the allowlist for
`base`, so placeholder validation passes unchanged.

### CLAUDE.md

Add a fifth bullet to the "Pedagogical principles encoded in the system
prompt" section, so the principle constrains future prompt/dialogue
changes:

> - When the child asserts a claim, the Primer asks how she knows or how she
>   could check — it does not confirm or correct outright. (`ProbeReasoning`
>   intent.)

## Layer 2 — the `ProbeReasoning` intent

### Enum (`primer-core/src/conversation.rs`)

Add variant `ProbeReasoning`, **appended at the end** of:

- the `enum PedagogicalIntent` declaration,
- the `ALL` slice,
- the `name()` match (`Self::ProbeReasoning => "ProbeReasoning"`).

Appending at the end preserves every existing variant's position, so the
storage lookup-table integer ids are untouched and `ProbeReasoning` takes
the next id. Doc comment on the variant: *"The child asserted a claim; ask
how she knows or how she could check, rather than confirming or correcting."*

Test updates in the same file: bump the `ALL.len()` assertion from 9 → 10,
add `name()` and `Display` assertions for the new variant.

### Pack mapping (`primer-pedagogy/src/prompt_pack.rs`)

- Add `PedagogicalIntent::ProbeReasoning` to the `ALL_INTENTS` slice
  (append at end, mirroring the enum order).
- Add the `intent_key` arm: `PedagogicalIntent::ProbeReasoning =>
  "probe_reasoning"`.

`intent_index` and `parse_intent_key` derive from these two automatically.

### Pack instruction (`[intent].probe_reasoning` in all three packs)

English reference draft:

> Your next response should ask the child how they know what they just
> claimed, or how they could check it — not whether it is right or wrong.
> Treat a confident assertion as an invitation to look again together, never
> as something to confirm or correct.

DE/HI get natural-register equivalents. The existing pack tests iterate
`PedagogicalIntent::ALL` asserting every `intent_instruction` is non-empty,
so a missing key in any pack fails the suite — no separate guard needed.

### Storage

The `pedagogical_intents` lookup table is seeded and validated against
`PedagogicalIntent::ALL` on every `open()`. Adding a variant auto-seeds the
new row on the next DB open — **no migration**, exactly as `SuggestBreak`
(id 9) required none.

## Layer 2 — routing (`decide_intent_at_with_pack`)

The new branch sits in the last-turn-is-a-child analysis, **after** the
factual-question branch and the short-answer branch, **before** the
`SocraticQuestion` default. It captures the case that currently falls
through to the default:

```text
if last child turn:
    if is_factual_question      -> DirectAnswer / AnswerThenPivot   (unchanged)
    if word_count < 10          -> ComprehensionCheck               (unchanged)
    compute has_understood
    if has_understood           -> Extension                        (unchanged)
    if is_assertion(last.text)  -> ProbeReasoning                   (NEW)
default                          -> SocraticQuestion
```

`is_assertion` is a small helper: the turn is a declarative claim, i.e. its
trimmed text does **not** end with `?`. (The factual-question branch already
diverts factual questions via early return; this guard additionally keeps
non-factual questions — "can you help me?" — out of the `ProbeReasoning`
route, leaving them on the `SocraticQuestion` default.)

So `ProbeReasoning` fires when **all** hold for the last child turn:

- it is a declarative assertion (no trailing `?`),
- it is `>= SHORT_TURN_WORD_BOUNDARY` (10) words — a substantive claim, not
  a one-word guess that `ComprehensionCheck` already handles,
- no active concept (over `ACTIVE_CONCEPT_LOOKBACK` = 4 turns) is at
  `Comprehension` or above — the child has asserted something they have not
  yet demonstrated understanding of.

Engagement-state overrides and the break gate continue to fire before any
turn analysis, so a frustrated child still gets `Scaffolding`, not
`ProbeReasoning`.

## Testing

Add characterization tests to `prompt_builder/tests.rs` (the 18-test suite
pinning intent routing):

1. Substantive declarative claim, not-yet-understood → `ProbeReasoning`.
2. Same claim phrased as a question (trailing `?`) → unchanged
   (`SocraticQuestion` default, or factual route if applicable).
3. Substantive claim with an active concept at `Comprehension`+ → still
   `Extension`.
4. Short declarative answer (< 10 words) → still `ComprehensionCheck`.
5. Frustrated engagement state + a substantive claim → still `Scaffolding`
   (override precedence).

The pack suite's existing `ALL`-iterating `intent_instruction` non-empty
check covers the new key in all three packs.

## Files touched

- `primer-core/src/conversation.rs` — enum variant, `ALL`, `name()`, tests.
- `primer-pedagogy/src/prompt_pack.rs` — `ALL_INTENTS`, `intent_key`.
- `primer-pedagogy/src/prompt_builder.rs` — `is_assertion` helper + routing
  branch.
- `primer-pedagogy/src/prompt_builder/tests.rs` — characterization tests.
- `primer-pedagogy/prompts/en.toml` — core-principle bullet + `[intent]`
  key.
- `primer-pedagogy/prompts/de.toml` — natural-register equivalents.
- `primer-pedagogy/prompts/hi.toml` — natural-register equivalents.
- `CLAUDE.md` — fifth pedagogical-principle bullet.

No schema migration. No new constants (reuses `SHORT_TURN_WORD_BOUNDARY`,
`ACTIVE_CONCEPT_LOOKBACK`).
