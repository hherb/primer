# Primer Inference Architecture — Local-First with Cloud Supervision

*Technical design document. May 2026.*

---

## The Scalability Problem

Commercial AI inference does not scale to universal education. The arithmetic is unforgiving.

A 15-minute Socratic conversation generates roughly 30 exchange pairs. At current cloud API pricing (Anthropic Sonnet-class), that costs approximately $0.10–0.30 per session depending on context length and system prompt size. For a single child, this is negligible. For the population that needs the Primer, it is impossible.

There are approximately 700 million primary-school-age children worldwide. If 10% used the Primer for one session per day — 70 million sessions — the daily inference cost at $0.15/session would be $10.5 million. Annually: $3.8 billion. For a free, non-profit educational tool. Even at 1% penetration, the cost is $380 million per year in perpetuity, with no mechanism to reduce it because the cost scales linearly with usage.

Cloud inference also creates dependencies that directly contradict the Primer's design principles: internet connectivity (absent in many target environments), data transit (violating the on-device privacy guarantee), and vendor lock-in (AGPL means nothing if the system can't function without a proprietary API).

Local inference on a $50–100 device running a 3–7B parameter model costs electricity. A few cents per day at most. The upfront hardware cost is real, but it is one-time, it drops every year, and it is the kind of cost that aid organisations, governments, and communities can absorb — the way they absorbed the cost of textbooks for a century.

**The Primer must be designed for local inference as the primary mode from the earliest architectural decisions. Cloud inference is a development convenience and a supervisory channel, not the production path.**

---

## The Two-Tier Architecture

The design maps onto how human educational systems actually work at their best: a classroom teacher handles daily interaction; a specialist is consulted periodically for things that exceed the teacher's capacity.

### Tier 1 — Local Model (The Teacher)

The local model runs on the child's device. It handles all real-time interaction:

- Socratic dialogue (question generation, follow-up, scaffolding)
- Engagement detection (frustration, boredom, flow — from response patterns, latency, vocabulary)
- Comprehension assessment (distinguishing understanding from parroting from confusion)
- Spaced repetition scheduling (which concepts and vocabulary to revisit, when)
- Concept extraction (identifying what the child discussed and at what depth)
- Session management (greeting, context recall from prior sessions, break suggestions, session close)
- Learner model updates (concept mastery, vocabulary tracking, engagement history)

**Model class:** 3–7B parameters, quantised (Q4_K_M or equivalent), running on consumer hardware. Current leading candidates: Qwen3 7B (MoE, high reasoning quality for size), Gemma 4 E2B (2.3B effective, dense, battery-efficient for edge), Gemma 4 26B A4B (MoE, 3.8B active, best quality-per-watt if hardware supports it).

**Latency target:** < 3 seconds to first token. Acceptable on current-generation phone SoCs (Snapdragon 8 Elite) and mid-range SBCs (Orange Pi 6 Plus / CIX P1).

**What the local model must do well:**

- Generate age-appropriate, contextually relevant Socratic questions
- Maintain conversational coherence across a session (20+ turns)
- Detect when a child's response indicates understanding vs parroting vs confusion
- Adjust difficulty and pedagogical intent in real time
- Refuse to answer when the child can reason through it (the core Primer behaviour)
- Handle the "silly claim" / epistemic provocation mode (present plausible falsehoods or implausible truths calibrated to the child)

**What the local model does not need to do:**

- Be encyclopedically accurate on all topics (the RAG knowledge base handles grounding)
- Perform deep multi-step reasoning on complex topics (escalate to cloud)
- Assess long-term learning trajectories across weeks/months (cloud supervisor)
- Generate curriculum plans or learning arcs (cloud supervisor)

### Tier 2 — Cloud Model (The Specialist)

The cloud model is consulted periodically, not in real time. It serves a supervisory and analytical role.

**When it is called:**

- **Scheduled review** (weekly or fortnightly): The local Primer packages a compressed summary of recent sessions — not raw transcripts, but a pedagogical digest: concepts explored, comprehension signals, engagement patterns, learner model state, any anomalies the local model flagged but couldn't resolve. The cloud model analyses this and returns course corrections.

- **Escalation on demand**: When the local model detects a situation it cannot handle well — a child asking about a topic outside the knowledge base, a persistent misconception the local model can't find the right question to surface, a comprehension pattern it can't classify — it flags the exchange for cloud review. If connectivity is available, this can happen in near-real-time (the child sees a brief pause, perhaps "Let me think about that for a moment..."). If not, it queues for the next scheduled sync.

- **Learner model audit** (monthly or quarterly): A deeper analysis of the child's learning trajectory. Is the learner model accurate? Are there blind spots — concepts the model marks as mastered that the child's responses suggest aren't? Are the spaced repetition intervals calibrated well for this specific child? The cloud model can compare against aggregate patterns from other children (anonymised, opt-in) to detect anomalies.

- **Curriculum generation** (as needed): When the local model's knowledge base doesn't cover a topic the child is pursuing, the cloud model can generate new knowledge base entries, Socratic question sequences, and age-appropriate explanations — which are then cached locally for future sessions.

**What is sent to the cloud:**

- Pedagogical summary (concepts, mastery levels, engagement scores, comprehension signals)
- Learner model state (anonymised — no child name, no location, no personal details)
- Flagged exchanges (specific turns where the local model was uncertain, with surrounding context)
- Never: raw full transcripts, child identity, family information, session recordings

**What is returned from the cloud:**

- Course correction suggestions (topic pivots, difficulty adjustments, new question strategies)
- Updated spaced repetition schedules
- New knowledge base entries for topics the child is exploring
- Learner model calibration adjustments
- Alerts for patterns that may indicate issues outside the Primer's scope (e.g., signs of distress, learning difficulties that need professional assessment — flagged to parents, never acted on autonomously)

**Bandwidth:** The pedagogical summary for a week of daily sessions is a few kilobytes. This could travel over SMS where that's the only connectivity available. The return payload is similarly small — text, not media. The architecture does not assume broadband.

**Cost:** One cloud API call per child per week (the supervisory review) costs roughly $0.02–0.05 with a capable model. At 70 million children, that's $1.4–3.5 million per week — still large, but an order of magnitude less than real-time cloud inference, and reducible further as the supervisory model itself can be a smaller specialist fine-tuned for pedagogical review rather than a general-purpose frontier model.

---

## Fine-Tuning Strategy for Local Models

Relying on local models means the quality ceiling is lower than cloud — unless the local models are specifically tuned for the Primer's tasks. A general-purpose 7B model that is mediocre at everything is less useful than a 3B model that is excellent at Socratic dialogue with children.

### Task Decomposition

The Primer's inference load is not a single task. It is several distinct tasks that could be served by different models or adapters:

| Task | Description | Quality requirement | Latency requirement |
|------|-------------|-------------------|---------------------|
| **Socratic dialogue** | Generate age-appropriate questions, follow-ups, scaffolding | High — this is the core product | < 3s to first token |
| **Comprehension assessment** | Classify child's response as understanding / parroting / confusion | Moderate — can be probabilistic | Can run async (1 turn behind) |
| **Engagement classification** | Detect frustration, boredom, flow, disengagement from response patterns | Moderate | Can run async |
| **Concept extraction** | Identify concepts discussed and estimate depth | Moderate | Can run async |
| **Summary generation** | Produce rolling session summaries for long-term memory | Moderate | End of session, not real-time |
| **Knowledge grounding** | Verify factual claims against knowledge base, generate grounded responses | High for factual accuracy | Inline with dialogue |
| **Epistemic provocation** | Generate plausible falsehoods or implausible truths calibrated to the child | High — must be convincing and age-appropriate | Can be pre-generated |

Several of these (comprehension assessment, engagement classification, concept extraction) are classification tasks that may be better served by small, specialised models or even non-LLM classifiers (fine-tuned BERT-class models, logistic regression over embeddings) rather than by the same model that generates dialogue. This reduces the load on the primary dialogue model and allows each task to be optimised independently.

### Training Data Requirements

Fine-tuning requires data. The Primer's AGPL licence and privacy principles constrain data collection, but also create opportunities.

**Data sources, in order of feasibility:**

1. **Synthetic data from cloud models.** Use a frontier model (Claude, GPT-4 class) to generate thousands of example Socratic dialogues across age groups, topics, and difficulty levels. This is the fastest path and requires no real children. The dialogues can be reviewed by educators for quality. This is the bootstrap — it gets the local model to "good enough" quality for initial deployment.

2. **Curated educator-authored dialogues.** Partner with teachers (Pratham, UNESCO pilot sites, individual collaborators) to produce gold-standard example dialogues. A teacher interacts with the Primer and annotates: "This question was good," "This follow-up missed the point," "The child's response here indicates confusion, not understanding." Expensive per example, but high quality. A few hundred annotated dialogues would be extremely valuable.

3. **Opt-in anonymised interaction data from pilot deployments.** Once the Primer is in use, parents can opt in (explicit, per-child, per-export consent — already designed in Phase 0.3) to contribute anonymised interaction data. This data is scrubbed on-device before transmission: child names, family references, locations, and personal details are stripped. What remains: age, topic, Socratic exchange patterns, comprehension signals, engagement patterns. This is the long-term data flywheel — the Primer improves because children use it, and the improvement benefits all children.

4. **The Phase 0.3 vocabulary corpus.** Already designed with anonymisation in mind (Phase 4 in the roadmap). Tuples of (age, technical term, plain-language explanation, child comprehension signal) contribute to a shared dataset for fine-tuning age-calibrated language models. This is a unique dataset — no one else has it — and it directly addresses the problem of models speaking above or below children's level.

5. **Published educational dialogue datasets.** Academic datasets from tutoring research (e.g., the CIMA dataset, the Socratic dialogue datasets from educational AI research). These tend to be small and skewed toward older students, but they're freely available and useful for initial training.

### Fine-Tuning Approaches

**LoRA / QLoRA adapters** are the most practical approach for local models. A base model (Qwen3 7B, Gemma 4) is augmented with small, task-specific adapter weights that can be swapped or combined:

- A "Socratic dialogue" adapter trained on synthetic + educator-authored dialogues
- A "comprehension assessment" adapter trained on annotated child responses
- An "age calibration" adapter per age band (5–7, 8–10, 11–12) trained on age-appropriate language patterns
- A "topic specialist" adapter per major knowledge domain (science, mathematics, language, social studies)

Adapters are small (tens of MB), can be distributed independently of the base model, and can be updated without replacing the entire model. This means the Primer can improve incrementally — a new adapter for a new topic, a refined comprehension classifier, a better age-calibration layer — without redeploying the core model.

**Distillation from cloud models** is a complementary approach. Use the cloud supervisory model to generate high-quality responses for cases where the local model performed poorly, then fine-tune the local model on these examples. This creates a feedback loop: the cloud model teaches the local model to handle cases it previously couldn't, gradually reducing the need for cloud escalation.

### Data Preparation Pipeline

A systematic data pipeline is needed from early development, not bolted on later.

**Phase 0 (now):**
- Begin generating synthetic training dialogues using Claude (the current cloud backend) across age groups, topics, and difficulty levels
- Develop annotation schema for dialogue quality (question relevance, scaffolding appropriateness, comprehension assessment accuracy, age calibration)
- Store all synthetic data in a structured format (JSONL with metadata: age, topic, pedagogical intent, quality rating)
- Establish a review workflow — at minimum, Horst reviewing synthetic dialogues for pedagogical quality

**Phase 0–1 (next 6 months):**
- Instrument the Primer CLI to optionally log interactions in training-data format (off by default, explicit opt-in, local storage only)
- Begin collecting Aiyana's (and other test children's) interactions as gold-standard evaluation data (with parental consent, stored locally, used only for model evaluation)
- Define evaluation benchmarks: given an input, does the fine-tuned model produce questions that are (a) age-appropriate, (b) Socratic (not didactic), (c) responsive to the child's actual answer, (d) at the right difficulty level?

**Phase 1 (local inference):**
- First fine-tuning experiments: LoRA adapters on the chosen base model using synthetic + early real data
- A/B evaluation: compare base model vs fine-tuned model on benchmark dialogues
- Iterate: generate more synthetic data targeting weak spots identified in evaluation

**Phase 2+ (pilot deployments):**
- Implement the opt-in anonymised data contribution pipeline (Phase 4 roadmap item, but the *infrastructure* should be designed now)
- Begin collecting the vocabulary corpus (age, term, explanation, comprehension signal)
- Federated fine-tuning: if multiple deployment sites contribute data, adapters can be trained on combined data without raw data leaving any site

---

## Inference Router Design

The current `InferenceBackend` trait (implemented by `StubBackend`, `CloudBackend`, `OllamaBackend`, with `LlamaCppBackend` planned) handles backend selection at startup via CLI flag. The two-tier architecture requires a more sophisticated router.

### Proposed Architecture

```
InferenceRouter
├── primary: Box<dyn InferenceBackend>     // local model (llama.cpp, QNN, RKNN)
├── supervisor: Option<Box<dyn InferenceBackend>>  // cloud model (Anthropic API)
├── classifiers: ClassifierBank            // small specialist models
│   ├── comprehension: Box<dyn ComprehensionClassifier>  // already exists as trait
│   ├── engagement: Box<dyn EngagementClassifier>        // already exists as trait
│   └── concept_extractor: Box<dyn ConceptExtractor>     // already exists as trait
├── escalation_policy: EscalationPolicy
└── sync_schedule: SyncSchedule
```

**The router's responsibilities:**

1. **Route real-time dialogue** through the primary (local) backend
2. **Run classifiers** asynchronously (one turn behind the dialogue, as currently designed)
3. **Detect escalation conditions** — topic outside knowledge base, comprehension assessment uncertainty above threshold, persistent misconception pattern, child explicitly asking something the local model can't handle
4. **Queue escalation requests** for the supervisor, with connectivity-aware delivery (immediate if online, queued if offline)
5. **Execute scheduled supervisory reviews** — package pedagogical summary, send to cloud, apply returned course corrections to the learner model and dialogue manager
6. **Fall back gracefully** — if the local model fails or is unavailable, the router should not silently switch to cloud (that violates the privacy guarantee). It should either use the stub backend (degraded but functional) or inform the child that the Primer needs a rest.

### Escalation Policy

The `EscalationPolicy` struct should be configurable by parents:

```rust
pub struct EscalationPolicy {
    /// Allow any cloud communication at all
    pub cloud_enabled: bool,
    /// Allow real-time escalation (vs batch-only)
    pub realtime_escalation: bool,
    /// Minimum confidence threshold for local comprehension assessment
    /// before escalation is considered (0.0–1.0)
    pub comprehension_confidence_threshold: f32,
    /// Maximum queued escalation requests before forcing a sync attempt
    pub max_queued_escalations: usize,
    /// Sync schedule
    pub sync_interval: SyncInterval,
}

pub enum SyncInterval {
    Daily,
    Weekly,
    Fortnightly,
    Manual,  // parent triggers sync explicitly
    Never,   // fully airgapped, no cloud communication
}
```

**The `Never` option must always work.** A Primer running in `SyncInterval::Never` mode must be fully functional — degraded in supervisory quality, but never broken. This is the design principle: local-first means local-always-works.

---

## Hardware Implications

The two-tier architecture has specific hardware implications for Phase 1 and Phase 3:

**Minimum viable local inference:**
- 4GB RAM (for a Q4-quantised 3B model with context)
- ARM SoC with NEON or equivalent SIMD (baseline CPU inference)
- 16GB storage (model weights + knowledge base + learner model + cached adapters)
- No GPU/NPU required (but beneficial — 2x–5x speedup)

**Recommended for good experience:**
- 8–16GB RAM (for a 7B model with longer context)
- NPU or GPU (Snapdragon Hexagon, Mali, Apple Neural Engine, Rockchip RKNN)
- 32GB storage (multiple model variants, larger knowledge base)

**Target devices (Phase 1):**
- Phones: Snapdragon 8 Elite class (RedMagic 11 Pro — already in pipeline for testing)
- SBCs: Orange Pi 6 Plus (CIX P1, 30 TOPS NPU) — already identified as candidate
- Tablets: mid-range Android with 8GB+ RAM (most affordable path to wide deployment)

**The phone-as-Primer question:** If a current-generation phone delivers acceptable inference quality and latency, the Primer could be a software-only product — no custom hardware needed. The phone provides compute, microphone, speaker, and battery. The Primer is an app. This dramatically lowers the deployment barrier. The Phase 3 custom enclosure remains valuable for younger children (drop-resistant, no distracting apps, physical volume knob), but it becomes optional rather than essential.

---

## Privacy Architecture

The two-tier model requires a clear, auditable data boundary.

**What never leaves the device:**
- Raw conversation transcripts
- Child's name, age, location, family details
- Audio recordings (if speech mode is used)
- Full learner model (concept mastery details, engagement history, session logs)

**What may leave the device (with explicit parental opt-in):**
- Anonymised pedagogical summary (concepts explored, mastery signals, engagement patterns — no identifying information)
- Anonymised vocabulary tuples (age band, term, explanation, comprehension signal — for the shared training corpus)
- Flagged exchanges (specific turns where the local model was uncertain — with surrounding context scrubbed of personal details)

**Scrubbing happens on-device.** The anonymisation pipeline runs locally before any data is transmitted. It is not a server-side process. The Primer does not trust the cloud to anonymise — it anonymises before the cloud ever sees the data.

**Parental consent is granular:**
- Consent for cloud supervisory sync (course corrections)
- Consent for anonymised data contribution (training corpus)
- Consent for escalation (real-time cloud queries)
- Each consent is per-child, revocable, and visible in the parental dashboard

---

## Development Priorities

This architecture implies several items that should be prioritised earlier than the current roadmap suggests:

1. **Synthetic training data generation (start now).** Every conversation the Primer has with Claude during development is potential training data for local models. Instrument the cloud backend to log interactions in fine-tuning format. Review and annotate the best examples.

2. **Evaluation benchmarks (start now).** Define what "good enough for local" means before selecting or fine-tuning a model. Age-appropriate? Socratic? Responsive? Difficulty-calibrated? These benchmarks drive everything else.

3. **Inference router design (Phase 0.4 or Phase 1.1).** The current backend selection is a CLI flag. The router should be designed before local inference lands, so the local backend integrates into the two-tier architecture from the start rather than being retrofitted.

4. **Anonymisation pipeline (Phase 0.3–0.4).** The on-device scrubbing pipeline should be built and tested before any pilot deployment, even if the data contribution feature isn't enabled yet. Building it early means it can be audited and refined before real children's data is involved.

5. **Adapter infrastructure (Phase 1).** The ability to load, swap, and update LoRA adapters independently of the base model should be part of the `LlamaCppBackend` design from the start. This enables incremental improvement without full model redeployment.

---

## Summary

The Primer's inference architecture is not a technical implementation detail — it is a values decision. Local-first inference means: no recurring cost per child, no internet dependency, no data leaving the device, no vendor lock-in, no scalability ceiling. Cloud supervision means: the local model improves over time, edge cases are handled, the learner model stays calibrated, and children in disconnected environments still get the benefit of frontier-model intelligence — just asynchronously.

The cost of this architecture is upfront investment in fine-tuning, training data, and evaluation — work that a cloud-only system avoids by renting intelligence per query. But the cloud-only system rents from a landlord whose prices it cannot control, whose capacity it cannot guarantee, and whose interests may not permanently align with universal free education. The Primer owns its intelligence. That is the point.
