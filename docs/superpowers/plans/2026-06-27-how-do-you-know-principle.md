# "How Do You Know?" Principle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bake the principle *"a child who is told she is wrong learns to defend; a child who is asked how she knows learns to look again"* into the Primer as both prompt text and a new `ProbeReasoning` pedagogical intent.

**Architecture:** Add a `ProbeReasoning` variant to `PedagogicalIntent` (appended at the end so existing on-disk integer ids are untouched), route to it from `decide_intent` when the last child turn is a substantive declarative claim the child has not yet shown they understand, and state the principle in the system-prompt core-principles list (all three locale packs) plus CLAUDE.md.

**Tech Stack:** Rust (edition 2024, toolchain 1.88), `cargo` run from `src/`. Three TOML prompt packs (`en`/`de`/`hi`).

## Global Constraints

- All `cargo` commands run from `/Users/hherb/src/primer/src` and use the rustup proxy: invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows the pinned 1.88 toolchain — see CLAUDE.md).
- New intent variant is **appended at the end** of the enum, `ALL`, and every match — never inserted mid-list. Existing integer ids (SocraticQuestion=1 … SuggestBreak=9) must not shift.
- No schema migration: the `pedagogical_intents` lookup table is validate-and-seeded from `PedagogicalIntent::ALL` on every DB `open()`.
- No magic numbers: reuse the existing `SHORT_TURN_WORD_BOUNDARY` (10) and `ACTIVE_CONCEPT_LOOKBACK` (4) consts.
- Prompt-pack `[intent]` values allow **no placeholders**; `[system_prompt].base` allows only `{name}`, `{age}`, `{language_guidance}`. Loader panics on violations.
- TOMLs are not mechanically translated — DE/HI get natural-register equivalents (exact strings supplied below).

---

## Task 1: Introduce the `ProbeReasoning` intent (enum + all match sites + pack keys)

Adding the variant breaks five exhaustive `match`es across three crates **and** makes every prompt-pack load fail (the loader hard-errors on a missing intent key). All of these must land together or the workspace will not compile / tests will not load packs. This task is the compile-coupled foundation; it adds no routing yet.

**Files:**
- Modify: `crates/primer-core/src/conversation.rs` (enum, `ALL`, `name()`, tests at lines 190, 198-214, 217-221)
- Modify: `crates/primer-core/src/router.rs:89-101` (`intent_weight`)
- Modify: `crates/primer-storage/src/catalog.rs` (`intent_id` ~line 30, `intent_name` ~line 46)
- Modify: `crates/primer-storage/src/store/tests/session_tests.rs:147` and `:311` (count `9` → `10`)
- Modify: `crates/primer-pedagogy/src/prompt_pack.rs:724-748` (`ALL_INTENTS`, `intent_key`)
- Modify: `crates/primer-pedagogy/prompts/en.toml` (`[intent]` table)
- Modify: `crates/primer-pedagogy/prompts/de.toml` (`[intent]` table)
- Modify: `crates/primer-pedagogy/prompts/hi.toml` (`[intent]` table)

**Interfaces:**
- Produces: `PedagogicalIntent::ProbeReasoning` (id `10`, `name()` `"ProbeReasoning"`, snake_case key `"probe_reasoning"`, router weight `0.25`). Consumed by Task 2 (routing) and Task 3 (prose only references it in docs).

- [ ] **Step 1: Add the variant and every compiler-forced match arm + pack keys (one edit pass — must compile before any test runs)**

In `crates/primer-core/src/conversation.rs`, append to the enum (after `SuggestBreak`, before the closing `}` at line 67):

```rust
    /// The child asserted a claim; ask how she knows or how she could
    /// check, rather than confirming or correcting it outright.
    ProbeReasoning,
```

Append to the `ALL` slice (after `Self::SuggestBreak,` at line 81):

```rust
        Self::ProbeReasoning,
```

Append to the `name()` match (after the `SuggestBreak` arm at line 98):

```rust
            Self::ProbeReasoning => "ProbeReasoning",
```

In `crates/primer-core/src/router.rs`, add to the `intent_weight` match (after the `SuggestBreak` arm at line 99):

```rust
        PedagogicalIntent::ProbeReasoning => 0.25,
```

In `crates/primer-storage/src/catalog.rs`, add to `intent_id` (after `SuggestBreak => 9,`):

```rust
        PedagogicalIntent::ProbeReasoning => 10,
```

and to `intent_name` (after the `SuggestBreak` arm):

```rust
        PedagogicalIntent::ProbeReasoning => "ProbeReasoning",
```

In `crates/primer-pedagogy/src/prompt_pack.rs`, append to `ALL_INTENTS` (after `PedagogicalIntent::SuggestBreak,` at line 733):

```rust
    PedagogicalIntent::ProbeReasoning,
```

and to `intent_key` (after the `SuggestBreak` arm at line 746):

```rust
        PedagogicalIntent::ProbeReasoning => "probe_reasoning",
```

In `crates/primer-pedagogy/prompts/en.toml`, add to the `[intent]` table (place it right after the `comprehension_check` block):

```toml
probe_reasoning = """\
Your next response should ask the child how they know what they just \
claimed, or how they could check it — not whether it is right or wrong. \
Treat a confident assertion as an invitation to look again together, \
never as something to confirm or correct.\
"""
```

In `crates/primer-pedagogy/prompts/de.toml`, add to the `[intent]` table (after `comprehension_check`):

```toml
probe_reasoning = """\
Deine nächste Antwort soll das Kind fragen, woher es weiß, was es gerade \
behauptet hat, oder wie es das überprüfen könnte — nicht, ob es richtig \
oder falsch ist. Behandle eine selbstbewusste Behauptung als Einladung, \
gemeinsam noch einmal hinzuschauen, niemals als etwas, das du bestätigen \
oder korrigieren musst.\
"""
```

In `crates/primer-pedagogy/prompts/hi.toml`, add to the `[intent]` table (after `comprehension_check`):

```toml
probe_reasoning = """\
तुम्हारी अगली प्रतिक्रिया बच्चे से यह पूछनी चाहिए कि उसने अभी जो कहा वह कैसे \
जानता/जानती है, या उसे कैसे जाँचा जा सकता है — यह नहीं कि वह सही है या ग़लत। \
एक आत्मविश्वासी कथन को साथ मिलकर फिर से देखने का निमंत्रण मानो, कभी ऐसी चीज़ \
नहीं जिसे बस सही ठहराना या सुधारना हो।\
"""
```

- [ ] **Step 2: Build and run the affected suites to see the pre-existing count assertions fail**

Run: `~/.cargo/bin/cargo test -p primer-core -p primer-storage -p primer-pedagogy`
Expected: compiles; FAILS in three places —
- `primer-core` `pedagogical_intent_all_lists_every_variant` (asserts `ALL.len() == 9`)
- `primer-storage` `session_tests` line 147 (`intent_count == 9`) and line 311 (`count == 9`)

(The pack-load tests should PASS — the three TOML keys were added in Step 1. If any pack test reports `missing intent 'probe_reasoning'`, a TOML key is missing; fix it.)

- [ ] **Step 3: Update the count/name assertions to reflect the new variant**

In `crates/primer-core/src/conversation.rs`, change line 190:

```rust
        assert_eq!(PedagogicalIntent::ALL.len(), 10);
```

Add a `ProbeReasoning` assertion to `pedagogical_intent_name_matches_canonical_strings` (after the `SuggestBreak` assertion at line 213):

```rust
        assert_eq!(PedagogicalIntent::ProbeReasoning.name(), "ProbeReasoning");
```

In `crates/primer-storage/src/store/tests/session_tests.rs`, change line 147:

```rust
    assert_eq!(intent_count, 10);
```

and line 311:

```rust
    assert_eq!(count, 10, "missing rows should have been seeded");
```

- [ ] **Step 4: Run the suites again — all green**

Run: `~/.cargo/bin/cargo test -p primer-core -p primer-storage -p primer-pedagogy`
Expected: PASS. (Catalog tests `every_intent_round_trips_through_id` and `intent_ids_are_unique` auto-cover the new id; pack tests `intent_instruction` non-empty auto-cover the new key.)

- [ ] **Step 5: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-core/src/conversation.rs src/crates/primer-core/src/router.rs \
        src/crates/primer-storage/src/catalog.rs \
        src/crates/primer-storage/src/store/tests/session_tests.rs \
        src/crates/primer-pedagogy/src/prompt_pack.rs \
        src/crates/primer-pedagogy/prompts/en.toml \
        src/crates/primer-pedagogy/prompts/de.toml \
        src/crates/primer-pedagogy/prompts/hi.toml
git commit -m "feat(pedagogy): add ProbeReasoning intent (enum, ids, pack keys)"
```

---

## Task 2: Route substantive declarative claims to `ProbeReasoning`

Add the `is_assertion` helper and a routing branch in `decide_intent_at_with_pack`, sitting after the `Extension` gate and before the `SocraticQuestion` default. Update the three existing characterization tests that this reclassifies, and add new ones for the new route and its guards.

**Files:**
- Modify: `crates/primer-pedagogy/src/prompt_builder.rs` (add `is_assertion` near `is_factual_question` at line 508; add branch in `decide_intent_at_with_pack` at lines 641-645)
- Modify: `crates/primer-pedagogy/src/prompt_builder/tests.rs` (update 3 tests; add 3 tests)

**Interfaces:**
- Consumes: `PedagogicalIntent::ProbeReasoning` from Task 1.
- Produces: `decide_intent` returns `ProbeReasoning` when the last child turn is a non-question (`!text.trim_end().ends_with('?')`), `>= SHORT_TURN_WORD_BOUNDARY` words, not a factual question, and has no active concept at `Comprehension`+.

- [ ] **Step 1: Update the three existing tests that the new branch reclassifies, and add the new tests (write them first — they will fail until Step 3)**

In `crates/primer-pedagogy/src/prompt_builder/tests.rs`:

Replace the `ten_word_child_turn_is_not_short` test (lines 250-264) with:

```rust
#[test]
fn ten_word_declarative_turn_returns_probe_reasoning() {
    // 10 words → not short; declarative (no trailing '?'); no understood
    // concept → the new "how do you know?" route fires instead of the
    // bare SocraticQuestion default.
    let learner = learner_with(EngagementState::Engaged, vec![]);
    let mut session = empty_session();
    session.add_turn(child_turn(
        "one two three four five six seven eight nine ten",
        vec![],
    ));
    assert_eq!(
        decide_intent(&learner, &session),
        PedagogicalIntent::ProbeReasoning,
    );
}
```

Replace `long_child_turn_with_concept_below_comprehension_returns_socratic_question` (lines 299-315) with:

```rust
#[test]
fn long_child_turn_with_concept_below_comprehension_returns_probe_reasoning() {
    // Recall < Comprehension, so the Extension gate stays closed and the
    // declarative claim routes to ProbeReasoning.
    let learner = learner_with(
        EngagementState::Engaged,
        vec![concept_at("gravity", UnderstandingDepth::Recall)],
    );
    let mut session = empty_session();
    session.add_turn(child_turn(
        "gravity pulls everything down toward the centre of the earth always",
        vec!["gravity".to_string()],
    ));
    assert_eq!(
        decide_intent(&learner, &session),
        PedagogicalIntent::ProbeReasoning,
    );
}
```

Replace `long_child_turn_with_unrelated_concept_returns_socratic_question` (lines 335-354) with:

```rust
#[test]
fn long_child_turn_with_unrelated_concept_returns_probe_reasoning() {
    // Active concept doesn't match any tracked concept_id → no Extension;
    // the declarative claim routes to ProbeReasoning.
    let learner = learner_with(
        EngagementState::Engaged,
        vec![concept_at("photosynthesis", UnderstandingDepth::Application)],
    );
    let mut session = empty_session();
    session.add_turn(child_turn(
        "I think gravity is what holds the planets together in space somehow",
        vec!["gravity".to_string()],
    ));
    assert_eq!(
        decide_intent(&learner, &session),
        PedagogicalIntent::ProbeReasoning,
    );
}
```

Add three new tests at the end of the `// ─── Factual-question routing` block (after `non_factual_short_turn_still_returns_comprehension_check`, line 442):

```rust
// ─── ProbeReasoning: substantive declarative claims ───────────────

#[test]
fn substantive_declarative_claim_not_understood_returns_probe_reasoning() {
    let learner = learner_with(EngagementState::Engaged, vec![]);
    let mut session = empty_session();
    session.add_turn(child_turn(
        "the moon is made of rock and dust and it pulls the sea",
        vec![],
    ));
    assert_eq!(
        decide_intent(&learner, &session),
        PedagogicalIntent::ProbeReasoning,
    );
}

#[test]
fn substantive_claim_phrased_as_question_stays_socratic() {
    // Same length, but a trailing '?' makes it a (non-factual) question,
    // so the assertion guard fails and it stays on the default.
    let learner = learner_with(EngagementState::Engaged, vec![]);
    let mut session = empty_session();
    session.add_turn(child_turn(
        "the moon is made of rock and dust and stuff right?",
        vec![],
    ));
    assert_eq!(
        decide_intent(&learner, &session),
        PedagogicalIntent::SocraticQuestion,
    );
}

#[test]
fn frustrated_with_substantive_claim_still_scaffolding() {
    // Engagement-state override precedes turn analysis, so a frustrated
    // child's substantive claim gets Scaffolding, not ProbeReasoning.
    let learner = learner_with(EngagementState::FrustratedStuck, vec![]);
    let mut session = empty_session();
    session.add_turn(child_turn(
        "the moon is made of rock and dust and it pulls the sea",
        vec![],
    ));
    assert_eq!(
        decide_intent(&learner, &session),
        PedagogicalIntent::Scaffolding,
    );
}
```

(The "still Extension" and "short → ComprehensionCheck" spec cases are already pinned by the existing `long_child_turn_with_understood_concept_returns_extension` and `short_child_turn_returns_comprehension_check` tests — no duplicates added.)

- [ ] **Step 2: Run the tests to confirm the new/updated ones fail**

Run: `~/.cargo/bin/cargo test -p primer-pedagogy probe_reasoning`
Expected: FAIL — `decide_intent` still returns `SocraticQuestion` for these declarative claims (the branch doesn't exist yet). The `_stays_socratic` and `_still_scaffolding` tests may already pass; the four `probe_reasoning` ones fail.

- [ ] **Step 3: Add the `is_assertion` helper and the routing branch**

In `crates/primer-pedagogy/src/prompt_builder.rs`, add the helper immediately after `is_factual_question_with_pack` (after line 508):

```rust
/// True when `text` is a declarative claim rather than a question.
///
/// Used to route a substantive, not-yet-understood child assertion to
/// `ProbeReasoning` ("how do you know?") instead of the Socratic-question
/// default. A trailing `?` (after trimming) marks a question, which stays
/// on the default path; factual questions are already diverted earlier in
/// `decide_intent_at_with_pack`.
fn is_assertion(text: &str) -> bool {
    !text.trim_end().ends_with('?')
}
```

In `decide_intent_at_with_pack`, insert the branch after the `Extension` gate (after line 643, the closing `}` of `if has_understood { ... }`) and before the child-turn block closes:

```rust
            // The child asserted a substantive claim they have not yet
            // shown they understand. Ask how they know / how they could
            // check, rather than defaulting to a fresh Socratic question.
            // (Questions — including non-factual ones — fail is_assertion
            // and stay on the default path below.)
            if is_assertion(&last.text) {
                return PedagogicalIntent::ProbeReasoning;
            }
```

- [ ] **Step 4: Run the full pedagogy suite — all green**

Run: `~/.cargo/bin/cargo test -p primer-pedagogy`
Expected: PASS, including the updated characterization tests and the three new ones.

- [ ] **Step 5: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-pedagogy/src/prompt_builder.rs \
        src/crates/primer-pedagogy/src/prompt_builder/tests.rs
git commit -m "feat(pedagogy): route substantive declarative claims to ProbeReasoning"
```

---

## Task 3: State the principle in the prompt and docs

Add the principle bullet to the core-principles list in all three packs, update the EN full-text snapshot, add the fifth bullet to CLAUDE.md's pedagogical-principles section.

**Files:**
- Modify: `crates/primer-pedagogy/prompts/en.toml` (`[system_prompt].base` core-principles list)
- Modify: `crates/primer-pedagogy/prompts/de.toml` (`base` Grundprinzipien list)
- Modify: `crates/primer-pedagogy/prompts/hi.toml` (`base` मूल सिद्धांत list)
- Modify: `crates/primer-pedagogy/src/prompt_builder/tests.rs:850` (`want` snapshot string)
- Modify: `CLAUDE.md` ("Pedagogical principles encoded in the system prompt" section)

**Interfaces:** none (prose only).

- [ ] **Step 1: Add the EN principle bullet**

In `crates/primer-pedagogy/prompts/en.toml`, in the `Your core principles:` list inside `[system_prompt].base`, insert a new bullet immediately after the line
`- When {name} answers, assess whether they genuinely understand or are guessing/parroting.`
and before `- If they understand: ...`:

```
- When {name} states something — right *or* wrong — do not confirm it or correct it outright. Ask how they know, or how they could check. A child who is told she is wrong learns to defend; a child who is asked how she knows learns to look again.
```

- [ ] **Step 2: Update the EN snapshot to expect the new bullet**

In `crates/primer-pedagogy/src/prompt_builder/tests.rs`, in the `want` string at line 850, insert the rendered bullet (with `{name}` → `Tester`) at the matching position — after `...assess whether they genuinely understand or are guessing/parroting.\n` and before `- If they understand:`. The inserted substring (note the leading `- ` and trailing `\n`):

```
- When Tester states something — right *or* wrong — do not confirm it or correct it outright. Ask how they know, or how they could check. A child who is told she is wrong learns to defend; a child who is asked how she knows learns to look again.\n
```

- [ ] **Step 3: Add the DE and HI principle bullets**

In `crates/primer-pedagogy/prompts/de.toml`, in the `Deine Grundprinzipien:` list, insert after
`- Wenn {name} antwortet, prüfe, ob das Verständnis echt ist oder nur geraten/nachgeplappert.`
and before `- Bei echtem Verständnis: ...`:

```
- Wenn {name} etwas behauptet — ob richtig oder falsch — bestätige oder korrigiere es nicht einfach. Frage, woher {name} das weiß oder wie man es überprüfen könnte. Ein Kind, dem man sagt, es liege falsch, lernt sich zu verteidigen; ein Kind, das gefragt wird, woher es etwas weiß, lernt noch einmal hinzuschauen.
```

In `crates/primer-pedagogy/prompts/hi.toml`, in the `तुम्हारे मूल सिद्धांत:` list, insert after
`- जब {name} उत्तर दे, तो देखो कि क्या वह सच में समझा/समझी है या केवल अनुमान/नकल कर रहा/रही है।`
and before `- अगर समझा/समझी है: ...`:

```
- जब {name} कुछ कहे — चाहे सही हो या ग़लत — उसे यूँ ही सही मत ठहराओ और न ही सीधे सुधारो। पूछो कि {name} यह कैसे जानता/जानती है, या इसे कैसे जाँचा जा सकता है। जिस बच्चे को कहा जाता है कि वह ग़लत है, वह बचाव करना सीखता है; जिस बच्चे से पूछा जाता है कि वह कैसे जानता है, वह फिर से देखना सीखता है।
```

- [ ] **Step 4: Run the pedagogy suite — snapshot and placeholder validation pass**

Run: `~/.cargo/bin/cargo test -p primer-pedagogy`
Expected: PASS, including `snapshot_canonical_prompt_locks_full_text`. If it fails, the diagnostic prints the first diverging byte — align the `want` string exactly (watch the em-dash `—`, the `*wrong*` asterisks, and the single `\n`).

- [ ] **Step 5: Add the CLAUDE.md principle bullet**

In `CLAUDE.md`, under "## Pedagogical principles encoded in the system prompt", add a fifth bullet to the list (after the "Comprehension is verified through transfer questions..." bullet):

```markdown
- When the child asserts a claim, the Primer asks how she knows or how she could check — it does not confirm or correct outright. A child who is told she is wrong learns to defend; a child who is asked how she knows learns to look again. Routed deterministically via `PedagogicalIntent::ProbeReasoning` (a substantive declarative claim not yet at `Comprehension` depth), and stated in every pack's core-principles list.
```

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-pedagogy/prompts/en.toml \
        src/crates/primer-pedagogy/prompts/de.toml \
        src/crates/primer-pedagogy/prompts/hi.toml \
        src/crates/primer-pedagogy/src/prompt_builder/tests.rs \
        CLAUDE.md
git commit -m "docs(pedagogy): state the how-do-you-know principle in packs + CLAUDE.md"
```

---

## Task 4: Full-workspace verification

**Files:** none (verification only).

- [ ] **Step 1: Full test suite**

Run: `~/.cargo/bin/cargo test`
Expected: PASS across the workspace (default features).

- [ ] **Step 2: Lint and format**

Run: `~/.cargo/bin/cargo clippy --workspace --all-targets && ~/.cargo/bin/cargo fmt --all -- --check`
Expected: no clippy warnings; formatting clean.

- [ ] **Step 3: Smoke-check the routing in the REPL (optional, manual)**

Run: `~/.cargo/bin/cargo run --bin primer -- --verbose`
Then type a substantive declarative claim (≥10 words, no `?`), e.g. `the moon is made of rock and dust and it pulls the sea`. Confirm the `[intent]` verbose line on stderr reads `ProbeReasoning` and the Primer's reply asks how you know / how you could check rather than confirming or correcting. Type `quit` to exit.

---

## Self-Review Notes

- **Spec coverage:** Layer-1 prose → Task 3 (packs + CLAUDE.md). Layer-2 enum/key/storage → Task 1. Layer-2 routing → Task 2. All five spec test cases covered (cases 1/2/5 new in Task 2; cases 3/4 pinned by existing tests, noted in Task 2 Step 1). The spec's "Files touched" list omitted `router.rs::intent_weight` — it is a compiler-forced fifth match site, added in Task 1 with weight `0.25` (matches `ComprehensionCheck`'s cognitive tier).
- **No placeholders:** every code/TOML/string block is literal and complete.
- **Type consistency:** `ProbeReasoning` / `"ProbeReasoning"` / `"probe_reasoning"` / id `10` / weight `0.25` used identically across all tasks.
