# Supertonic 3 weights — OpenRAIL-M licence assessment (issue #170 gate)

**Date:** 2026-06-25
**Scope:** the *model weights* of [`Supertone/supertonic-3`](https://huggingface.co/Supertone/supertonic-3), as fetched at runtime by the Stage D auto-download path. **Not** the vendored Rust code (that is MIT — see below).
**Why this exists:** issue #170's risk register requires "a deliberate licence read before any default-path flip" because the weights are OpenRAIL-M, not MIT. This document is that read.
**Status:** ✅ **Licence gate cleared, conditionally** — a default-path flip is permissible subject to the four conditions in [Conclusion](#conclusion). This is an engineering assessment for the maintainer, **not legal advice**; the final call is the owner's, and the authoritative text is the `LICENSE` file in the HF model repo (re-read it before any production flip).

## The two-licence split

Supertonic ships under **two separate licences**, and conflating them is the trap this assessment exists to avoid:

| Artifact | Where | Licence | Restrictions relevant to us |
|---|---|---|---|
| **Inference code** | vendored at [`src/vendor/supertonic-rs/`](../../src/vendor/supertonic-rs/) (LICENSE = MIT) | **MIT** | None. Permissive; compatible with the workspace and with redistribution inside the AGPL repo. |
| **Model weights** | the four ONNX files + voice styles, downloaded at runtime from HF | **BigScience OpenRAIL-M** | Use-based restrictions + redistribution/pass-through/attribution obligations (below). |

The code is already vendored and unproblematic. **Everything below is about the weights**, which are deliberately *not* vendored — they are fetched at first use from Supertone's canonical HF repo (Stage D), so the AGPL repo never contains or redistributes them.

## What OpenRAIL-M is

"RAIL" = Responsible AI Licence; "-M" = the *Model* variant (weights, not source or data). Supertonic uses the **BigScience Open RAIL-M** text (the same family as BLOOM). It is a **permissive commercial licence with a behavioural carve-out**: royalty-free use *including commercial use*, but with an enforceable list of prohibited use cases that flow downstream to all users. It is **not** an OSI-approved open-source licence — the use restrictions make it "ethical-source," which has a concrete consequence for the AGPL posture (see [AGPL interaction](#agpl-interaction-keep-the-weights-distinct)).

## Use-based restrictions (Attachment A) vs. a children's Socratic tutor

The licence prohibits using the model to do any of the following. Each is assessed against the Primer's actual behaviour (a local-first Socratic tutor that synthesises spoken replies to a child).

| # | Prohibited use | Implicated? | Assessment |
|---|---|---|---|
| (a) | Violate applicable law / regulation | No | The product is a lawful educational tool. |
| (b) | **Exploit or harm minors** | **Touches the product, satisfied by design** | The Primer's *entire purpose* is to help, not exploit, minors. The pedagogy is explicitly non-engagement-maximising (offers breaks, never guilt). This clause is a reason to *keep* those guardrails, not a blocker — but it makes the "no dark patterns" pedagogical principles a **licence obligation**, not just a design preference. |
| (c) | Generate false info *to harm others* | No | N/A to a tutor; and the Socratic design probes rather than asserts. |
| (d) | Disseminate PII to harm individuals | No | All learner data is local; nothing is disseminated. |
| (e) | **Generate machine content without expressly and intelligibly disclaiming it is AI-generated** | **Yes — the one clause needing a deliberate product decision** | The synthesised voice *is* AI-generated content delivered to a child. The Primer already presents itself as "an AI learning companion," but clause (e) wants an *express, intelligible* disclaimer. See [Condition 2](#conclusion). |
| (f) | Defame / harass | No | N/A. |
| (g) | **Impersonation / deepfakes without consent** | No | The voices (M1–M5, F1–F5) are synthetic *catalogue* voices, not clones of identifiable real people. No consent issue. |
| (h) | Fully automated decisions adversely affecting legal rights | No | N/A. |
| (i) | Discriminate on social behaviour / predicted characteristics | No | N/A. |
| (j) | Exploit vulnerabilities of a specific group to distort behaviour and cause harm | **Touches the product, satisfied by design** | Children are a vulnerable group; the no-engagement-maximisation + break-suggestion design is exactly the posture this clause demands. Same status as (b). |
| (k) | Discriminate on protected characteristics | No | N/A. |
| (l) | **Provide medical advice / interpretation** | **Touches the product, satisfied by design** | A Socratic tutor must not give medical advice. Already aligned with the pedagogy; keep it a content boundary in the system prompt. |
| (m) | Generate info for justice / law-enforcement / immigration / asylum administration | No | N/A. |

**Net:** Of the 13 restrictions, **none is breached by the Primer's intended use.** Three (b, j, l) are *satisfied by the existing pedagogical design* and become licence obligations the design already meets. **One (e) — the AI-disclosure clause — requires a deliberate product decision** (a clear "this is an AI voice" disclosure), which is cheap and is the only new action item the use-restrictions produce.

## Redistribution, pass-through, attribution

If a distributor conveys the weights (or a derivative), OpenRAIL-M requires them to:

1. **Include the use restrictions as enforceable provisions** in any downstream agreement;
2. **Provide recipients a copy of the licence**;
3. **Mark modified files prominently** (we do not modify the weights);
4. **Retain copyright / attribution notices**;
5. **Require all downstream users to comply** with the use restrictions (the pass-through).

**How the Primer's runtime-download posture interacts with this — the load-bearing point:**

- The Primer **does not redistribute the weights.** Stage D downloads them from Supertone's canonical HF repo at first use; the end user receives them from Supertone, under Supertone's terms, not from us. This keeps obligations (1)–(4) on Supertone's side, not ours, and is the cleanest possible posture. **Do not mirror, bundle, or vendor the weights** — that would convert us into a distributor and pull all five obligations onto the Primer.
- The **pass-through (5)** still warrants a light touch even when we don't redistribute: because we *direct* a child's device to use the weights, the Primer's own terms/credits should (a) surface the OpenRAIL-M attribution and a link to the licence, and (b) bind end users to the use restrictions. This is satisfied by an attribution/credits entry plus a one-line clause in the Primer's user-facing terms — see [Condition 3](#conclusion). The existing per-source attribution machinery (`sources` table, "Powered by …" credit) is the natural home for the credit line.

## AGPL interaction — keep the weights distinct

The Primer's *code* is AGPL. OpenRAIL-M's use restrictions are **incompatible with the OSI Open Source Definition** (which forbids field-of-use restrictions), so:

- The weights **cannot be relicensed under AGPL** and **must not be described as "open source."** Describe them as "OpenRAIL-M licensed" / "openly licensed with use restrictions."
- Keeping the weights **out of the repo** (download-at-runtime) is what keeps the AGPL distribution clean — the AGPL source tree contains only MIT code (the vendored inference wrapper) and our own AGPL code. No copyleft/ethical-source conflict arises because the two artifacts are never combined into one distributed package.
- This is the *same* aggregation posture we already use for Whisper/Piper model files (downloaded, not vendored), so it introduces no new pattern.

## What this assessment clears — and what it does NOT

**Cleared:** the OpenRAIL-M licence is **not a blocker** for making Supertonic the default TTS (including the Hindi default-path flip), subject to the four conditions below. The #170 risk-register item "OpenRAIL-M weights … needs a deliberate licence read before any default-path flip" is **resolved** by this document.

**Explicitly NOT cleared by this document** (these are separate gates, tracked elsewhere):

- **Hindi prompt-pack native-speaker review** — `prompts/hi.toml` is machine-translated with `# REVIEW:` blocks. This gates the `Locale::ALL` flip independently of the licence and is **not** something this assessment touches. See [`docs/localisation/hi/README.md`](../localisation/hi/README.md).
- **In-loop A/B latency/quality numbers (Stage E)** — hardware/audio-bench gated.
- **Hindi corpus + retrieval benchmarks** — no Hindi corpus exists yet.

## Conclusion

A default-path flip to Supertonic weights is **licence-permissible** provided:

1. **Do not redistribute the weights.** Fetch them at runtime from Supertone's canonical HF repo only; never mirror, bundle, or vendor them. (Already the Stage D design.)
2. **Surface an express AI-voice disclosure** to the child/parent (clause e) — e.g. a one-line "the Primer speaks with an AI-generated voice" in onboarding or the voice-mode UI. Cheap; the only new product action item.
3. **Surface OpenRAIL-M attribution + use-restriction pass-through** in the Primer's credits/terms (a link to the licence and a short "you agree not to use this for …" clause), reusing the existing attribution machinery.
4. **Keep the weights legally distinct from the AGPL code** — never call them "open source"; describe them as OpenRAIL-M-licensed with use restrictions.

None of (1)–(4) is onerous; (1) and (4) are already the design, (2) and (3) are small UI/docs additions. **Recommendation: proceed** when the *other* (non-licence) Hindi gates clear.

## Sources

- Model card: <https://huggingface.co/Supertone/supertonic-3> (license tag: `openrail`).
- Weights licence text: `LICENSE` in that repo (BigScience Open RAIL-M; Attachment A use restrictions (a)–(m) as enumerated above). Read 2026-06-25.
- Code licence: [`src/vendor/supertonic-rs/LICENSE`](../../src/vendor/supertonic-rs/LICENSE) (MIT).
- Stage A.5 spike (latency/coverage, incl. Hindi): [`supertonic3-stage-a5-spike.md`](supertonic3-stage-a5-spike.md).
