# The Primer Project — Technical Specification v0.1

**A Socratic AI Learning Companion for Children**

*April 2026*

---

## 1. Vision

A handheld, battery-powered device — roughly the size and weight of a hardback children's book — that serves as a personal AI learning companion. Inspired by Neal Stephenson's *A Young Lady's Illustrated Primer* in *The Diamond Age*, the device guides children through Socratic questioning, answers their questions with patience and depth, adapts to their understanding, and knows when they have genuinely grasped a concept versus merely parroting an answer.

The device must work offline for extended periods (remote areas, travel, network outages) while also leveraging cloud-based models when connectivity is available. It must be robust, child-safe, and designed for years of daily use.

**Design philosophy:** The Primer does not teach children what to think. It teaches them how to think — by asking questions, following their curiosity, demanding they articulate their understanding, and never accepting "I don't know" as a final answer when it senses the child is capable of reasoning further.

---

## 2. Target Users

**Primary:** Children aged 5–14, with interface and content adapting to developmental stage.

**Secondary:** Parents and educators who configure learning goals, review progress, and participate in the child's learning journey.

**Deployment contexts:**
- Home use (developed world, reliable connectivity)
- School/educational settings
- Remote and regional areas (intermittent or no connectivity)
- Travel (fully offline operation)

---

## 3. Hardware Architecture

### 3.1 Compute Core

**Recommended SBC:** Orange Pi 5 Plus 32GB (LPDDR4x) or Radxa ROCK 5T 32GB (LPDDR5)

Both use the Rockchip RK3588 SoC:
- 8-core CPU (4× Cortex-A76 @ 2.4GHz + 4× Cortex-A55 @ 1.8GHz)
- Mali-G610 MP4 GPU
- 6 TOPS integrated NPU (RKNN)
- 32GB RAM (critical for concurrent LLM + STT + TTS pipelines)
- M.2 slot(s) for NVMe storage and AI accelerator

**Rationale for 32GB:** Concurrent inference pipelines (LLM + speech + vision) plus OS overhead, knowledge base caching, and conversation history buffering require more than 16GB for reliable operation without swap thrashing.

**Board comparison:**

| Feature | Orange Pi 5 Plus 32GB | Radxa ROCK 5T 32GB |
|---|---|---|
| Price | ~$150–170 | ~$220 |
| RAM type | LPDDR4x (44 GB/s) | LPDDR5 (51.2 GB/s) |
| M.2 slots | 1× M.2 M-key (NVMe), 1× M.2 E-key (WiFi) | 2× M.2 M-key, 1× M.2 E-key |
| Advantage | Cost, availability | Higher bandwidth, dual M.2 for NVMe + accelerator |

**Recommendation:** ROCK 5T for prototype development (dual M.2 allows NVMe SSD + RK1828 accelerator simultaneously). Orange Pi 5 Plus for cost-sensitive production, using an NVMe-to-M.2 adapter or USB-attached accelerator.

### 3.1b Alternative Compute Core — Orange Pi 6 Plus (CIX P1)

**Added May 2026.** The Orange Pi 6 Plus is a significant upgrade path worth evaluating as prices drop.

**SoC:** CIX P1 (CD8180/CD8160)
- 12-core CPU: 4× Cortex-A720 @ 2.6GHz + 4× Cortex-A720 @ 2.4GHz + 4× Cortex-A520 @ 1.8GHz, 12MB L3 cache
- Arm Immortalis-G720 MC10 GPU (hardware ray tracing, Vulkan 1.3)
- 30 TOPS dedicated NPU (INT4/8/16, FP16, TF32) — 45 TOPS combined with CPU/GPU co-scheduling
- Up to 64GB LPDDR5 (128-bit bus, 5500 MT/s, ~88 GB/s theoretical bandwidth)
- Dual M.2 NVMe slots, USB 3.0, dual 5Gb Ethernet, WiFi 6, BT 5.0

**Key advantage over RK3588 + RK1828 stack:** The integrated 30 TOPS NPU may eliminate the need for a separate RK1828 accelerator entirely, simplifying the BOM and software stack (single NPU runtime instead of split inference across host and accelerator). The ~88 GB/s memory bandwidth is roughly double the RK3588's LPDDR4x bandwidth, directly improving LLM token generation speed (which is memory-bandwidth-bound).

**Power trade-off:** Reviews report 5–7W idle and 15–30W under load — higher than the RK3588. However, eliminating the RK1828 (3–5W active) partially compensates. Net power increase is modest. See updated power budget in §3.8.

**Current cost:** ~AUD $2000 for the 64GB configuration (May 2026). Too expensive for mass-market deployment, but acceptable for prototyping. At the current trajectory of ARM SBC pricing, comparable boards should reach $200–300 within 2–3 years.

**Board comparison (expanded):**

| Feature | ROCK 5T 32GB | Orange Pi 5 Plus 32GB | Orange Pi 6 Plus 64GB |
|---|---|---|---|
| Price (AUD) | ~$350 | ~$250 | ~$2000 |
| SoC | RK3588 | RK3588 | CIX P1 (CD8180) |
| CPU cores | 8 (A76+A55) | 8 (A76+A55) | 12 (A720+A520) |
| Integrated NPU | 6 TOPS | 6 TOPS | 30 TOPS (45 combined) |
| RAM | 32GB LPDDR5 | 32GB LPDDR4x | 64GB LPDDR5 |
| RAM bandwidth | ~51 GB/s | ~44 GB/s | ~88 GB/s |
| Needs RK1828? | Yes | Yes | Probably not |
| M.2 slots | 2× M-key | 1× M-key | 2× M-key |

**Recommendation updated:** The Orange Pi 6 Plus is the preferred prototyping platform if budget permits, due to the integrated NPU eliminating the RK1828 dependency and double the memory bandwidth. The RK3588-based boards remain the cost-effective baseline.

### 3.2 LLM Accelerator Module

**Recommended:** Rockchip RK1828 M.2 module

- 20 TOPS (INT8) dedicated NPU
- 5GB dedicated RAM (sufficient for 7B model weights in quantised format)
- M.2 2280 form factor
- Benchmarked at 59–180 tokens/sec on Qwen2.5/Qwen3 3B–7B models

**This is the critical enabler.** Without the RK1828, the bare RK3588 achieves only ~2–5 tok/s on a 7B model — too slow for conversational interaction with a child. With the RK1828, a 7B model runs at 60+ tok/s, enabling genuinely responsive Socratic dialogue.

**Fallback option:** If the RK1828 is unavailable or cost-prohibitive for initial prototyping, a Hailo-8L (13 TOPS) or Coral M.2 accelerator can handle smaller models (1.5B–3B) at acceptable speed, with the on-board RK3588 NPU handling speech pipelines.

### 3.3 Storage

**256GB NVMe SSD** (M.2 2242 or 2280 depending on board layout)

Approximate storage budget:
- LLM weights (7B Q4): ~5 GB
- Smaller fallback model (3B Q4): ~2 GB
- Whisper speech-to-text model: ~0.5 GB
- TTS voice models (multiple languages): ~1 GB
- Vision model (MobileViT or SigLIP): ~0.5 GB
- Compressed English Wikipedia: ~22 GB
- Curated children's encyclopedia/knowledge base: ~10 GB
- Curriculum content, books, illustrations: ~20 GB
- Conversation history and learner profiles: ~5 GB
- OS, applications, workspace: ~15 GB
- **Total: ~81 GB** (256GB gives ample room for expansion)

### 3.4 Display

**Primary recommendation:** 7.8" colour E Ink display with capacitive touch

- Gallery-series (E Ink Kaleido 3 or Gallery 4000) for colour illustrations
- 300 PPI for text readability
- Capacitive touch for child interaction (tap, swipe, draw)
- Optional: Wacom digitiser layer for stylus input (older children, writing/drawing)
- Power draw: ~0.5W during refresh, near-zero when static

**Prototyping shortcut:** Use a Boox Tab Mini C (7.8" colour E Ink, Android, USB-C) as display peripheral connected to the compute board via USB or local WiFi. This avoids custom display driver development during early prototyping while providing the exact form factor and interaction model of the final product.

**Alternative for richer interaction:** A low-power RLCD (reflective LCD) display offers faster refresh rates than E Ink while maintaining outdoor readability and low power draw. The Daylight Computer's DC-1 has demonstrated this approach. Trade-off: higher power consumption (~2W vs 0.5W), but much better for animated content and video.

### 3.5 Audio

**Microphone:** MEMS microphone array (2-mic minimum for basic noise rejection)
- I2S or USB interface
- Recommended: ReSpeaker 2-Mic HAT or equivalent
- Far-field capability for natural conversational distance (30–100cm)

**Speaker:** 3W mono speaker driver (adequate for voice; stereo not necessary)
- Class-D amplifier, I2S interface
- Housed in case with acoustic chamber for clarity

**Audio processing:**
- Hardware echo cancellation (AEC) for barge-in capability — child can interrupt
- Automatic gain control (AGC) for varying distances
- Voice activity detection (VAD) on RK3588 NPU

### 3.6 Camera (Optional, Phase 2)

- Low-resolution camera (2–5 MP) for object recognition, reading text, identifying plants/animals
- Not for surveillance — local processing only, no images stored or transmitted
- Activates only on explicit child request ("What is this?")

### 3.7 Connectivity

- WiFi 6 (802.11ax) — primary connectivity for cloud model access
- Bluetooth 5.0 — peripheral connectivity
- Optional: 4G/LTE module for mobile connectivity (field deployment scenarios)
- **All connectivity is optional.** The device must function fully offline.

### 3.8 Power

**Battery:** 100 Wh lithium-polymer pack (~500g, fits in a hardback book form factor)

Power budget:

**Option A: RK3588 + RK1828 stack**

| Component | Active (W) | Idle (W) |
|---|---|---|
| RK3588 SoC | 8–12 | 2 |
| RK1828 accelerator | 3–5 | 0.5 |
| E Ink display | 0.5 (refresh) | ~0 |
| Audio (mic + speaker) | 1 | 0.2 |
| NVMe SSD | 1–2 | 0.5 |
| WiFi (when active) | 1 | 0 |
| **Total** | **14.5–21.5** | **3.2** |

**Option B: Orange Pi 6 Plus (CIX P1) — no separate accelerator**

| Component | Active (W) | Idle (W) |
|---|---|---|
| CIX P1 SoC (incl. NPU) | 15–25 | 5–7 |
| E Ink display | 0.5 (refresh) | ~0 |
| Audio (mic + speaker) | 1 | 0.2 |
| NVMe SSD | 1–2 | 0.5 |
| WiFi (when active) | 1 | 0 |
| **Total** | **18.5–29.5** | **5.7–7.7** |

Note: CIX P1 power figures are from early reviews and vary considerably (some report idle as low as 4.2W, others up to 15W). Aggressive clock/core management during Primer idle periods could bring active power closer to Option A. The elimination of the RK1828 partially offsets the higher SoC power draw.

**Estimated battery life (100Wh battery):**

| Usage pattern | Option A (RK3588) | Option B (CIX P1) |
|---|---|---|
| Active conversation | 5–7 hours | 3–5 hours |
| Mixed use | 8–12 hours | 5–8 hours |
| Standby | 30+ hours | 13–18 hours |

**Charging:** USB-C PD, 45W for fast charging (0–80% in ~90 minutes)

### 3.9 Physical Enclosure

**Target dimensions:** 200 × 140 × 25mm (roughly B5/hardback size)
**Target weight:** < 800g including battery
**Materials:** Injection-moulded recycled ABS with rubberised edges (child-drop-resistant)
**IP rating:** IPX4 minimum (splash-proof)
**Thermal:** Passive cooling via aluminium heat spreader and case-integrated heat sinks. No fans (noise, dust, moving parts).

---

## 4. Software Architecture

### 4.1 Operating System

**Base:** Armbian (Debian-based) or Ubuntu for RK3588; Ubuntu or vendor-supplied Debian for CIX P1

The OS should be minimal — headless server with a custom application layer. No desktop environment. The entire UI runs through the Primer application. Note: CIX P1 Linux support is newer than RK3588's — kernel and driver maturity should be evaluated during prototyping.

### 4.2 Inference Engine

**Primary:** RKNN-LLM toolkit for RK1828-accelerated inference
- Rockchip's native toolkit for NPU-accelerated LLM inference
- Supports W8A8 quantisation on RK3588, W4A16 on RK1828
- Direct integration with Qwen, Llama, and other supported architectures

**Fallback:** llama.cpp for CPU-based inference on the RK3588 (or CIX P1)
- Used when dedicated NPU is unavailable or for secondary models
- Lower throughput (~3–5 tok/s on 7B for RK3588) but functional

**Alternative runtime (CIX P1):** If using the Orange Pi 6 Plus, the CIX NPU runtime replaces RKNN-LLM. The 30 TOPS integrated NPU should handle 7B models at conversational speed without a separate accelerator. Runtime maturity should be evaluated during prototyping — the CIX toolchain is newer than Rockchip's.

### 4.2b Model Candidates

**Added May 2026.** The Gemma 4 family (Google DeepMind, Apache 2.0) is particularly well-suited to the Primer's requirements:

**Gemma 4 E2B** — Dense "Edge" model, 5.1B total parameters, 2.3B effective. Designed specifically for on-device inference. At Q4 quantisation: ~1.5–2GB weights, runs comfortably on any of the candidate hardware platforms including phones. Suitable for battery-saving mode and younger children's simpler interactions.

**Gemma 4 26B A4B** — Mixture-of-Experts. 26B total parameters but only 3.8B active per forward pass. Inference cost and latency behave like a ~4B model, but reasoning quality draws from the full 26B parameter pool. Scores 88.3% on AIME 2026 maths benchmarks — competitive with models 20× its active size. At Q4 quantisation: ~14–16GB for weights, fits in 24GB (phone) or 32–64GB (SBC). This is the leading candidate for the Primer's primary offline model — the Socratic reasoning quality substantially exceeds a true 4B model, which matters for the pedagogical mission.

**Licensing:** All Gemma 4 variants are Apache 2.0, which is critical for a device intended to ship without cloud dependency or licensing entanglements.

**Model selection strategy (updated):**

| Scenario | Model | Hardware | Est. RAM for weights | Expected quality |
|---|---|---|---|---|
| Primary (offline) | Gemma 4 26B A4B (Q4) | CIX P1 NPU or RK1828 | ~14–16 GB | Excellent (MoE, 3.8B active) |
| Primary alt (offline) | Qwen3 7B (Q4) | RK1828 or CIX P1 NPU | ~5 GB | Good |
| Lightweight (battery save) | Gemma 4 E2B (Q4) | Any NPU or CPU | ~1.5–2 GB | Adequate for simple dialogue |
| Lightweight alt | Qwen3 3B (Q4) | RK1828 or CIX P1 NPU | ~2 GB | Adequate |
| Cloud (connected) | Claude Sonnet / Opus via API | Cloud | N/A | Best (streaming) |
| Emergency fallback | Qwen3 1.5B (Q4) | RK3588 NPU or CPU | ~1 GB | Minimal |

**Key insight (May 2026):** MoE architectures like Gemma 4 26B A4B are a game-changer for the Primer. Two years ago, Socratic dialogue quality required 70B+ parameters. Now, a model with 3.8B active parameters is competitive. This trajectory suggests that by the time the Primer reaches mass deployment, equivalent or better models will run on commodity phones — potentially shifting the Primer from a custom hardware product to a software product with recommended consumer devices.

### 4.3 Speech Pipeline

**Speech-to-Text:** Whisper (small or medium)
- Runs on RK3588 CPU/NPU
- Multilingual support
- Real-time factor < 0.5 (processes speech faster than it arrives)

**Text-to-Speech:** Piper TTS
- Lightweight, runs on RK3588 CPU
- Multiple voice options (warm, patient, gender-neutral default)
- SSML support for expressive narration (stories, emphasis)
- Low latency: < 200ms to first audio

**Conversation management:**
- Wake word detection (custom, trained for child voices) OR always-listening with VAD
- Barge-in support (child can interrupt without waiting for device to finish speaking)
- Silence detection and prompting ("Are you still thinking? Take your time.")

### 4.4 Vision Pipeline (Phase 2)

**Model:** MobileViT or quantised SigLIP encoder
- Runs on RK3588 NPU
- Object recognition, text reading (OCR), basic scene understanding
- < 500 MB RAM overhead

**Use cases:**
- "What is this?" — child shows an object to the camera
- Reading assistance — camera reads text from a book, sign, or label
- Nature identification — plants, animals, rocks, clouds
- Drawing interpretation — child draws something and asks questions about it

### 4.5 Knowledge Base

**Offline knowledge corpus:**
- Compressed English Wikipedia (full text, ~22 GB)
- Curated children's encyclopedia (age-appropriate, cross-referenced)
- Science curriculum materials (aligned to major national standards)
- Literature corpus (public domain children's books, fables, poetry)
- Mathematical concept library with progressive problem sets
- Historical timelines and geography data
- Basic medical/health information (age-appropriate first aid, body knowledge)

**Storage format:** SQLite + full-text search (FTS5) for fast retrieval. The LLM uses RAG (retrieval-augmented generation) to ground its responses in factual content from the knowledge base rather than relying solely on parametric knowledge.

**Update mechanism:** When connected to WiFi, the device syncs updated knowledge base segments from a content server. Updates are delta-compressed and verified. Parents/educators can curate content packs.

### 4.6 The Pedagogical Engine (Core Innovation)

This is the software layer that makes the Primer more than a chatbot. It is the primary research and development challenge.

**4.6.1 Learner Model**

A persistent representation of each child's knowledge state, maintained locally:

- **Concept graph:** A knowledge graph tracking which concepts the child has encountered, understood, partially understood, or not yet encountered. Edges represent prerequisite relationships.
- **Understanding depth:** For each concept, a multi-level assessment: recall (can repeat), comprehension (can explain in own words), application (can use in new context), analysis (can break down and reason about), synthesis (can combine with other concepts).
- **Learning style profile:** Observed preferences — does this child learn better through stories, through questions, through hands-on exploration, through visual aids? Adapts over time.
- **Engagement patterns:** Time of day, session length, topics that sustain attention, topics that lose it. Used to optimise session timing and topic sequencing.
- **Emotional state estimation:** (Cautious implementation) Basic sentiment detection from voice tone and response patterns. Used only to detect frustration, confusion, or disengagement — never to manipulate. Response: "I can tell this is tricky. Want to try a different way in?"

**4.6.2 Socratic Dialogue Manager**

The core interaction loop:

1. **Child asks a question or the device poses one**
2. **The device does not answer directly.** Instead, it asks a guiding question that leads the child toward discovering the answer themselves.
3. **The child responds.** The device assesses whether the response demonstrates understanding or is merely parroting.
4. **If understanding is partial:** Ask a follow-up that targets the gap. "You said X — can you explain why X is true?" or "What would happen if X were different?"
5. **If the child is frustrated:** Offer a concrete example, an analogy, or a story. Drop the abstraction level. "Let me tell you about a time when..."
6. **If the child demonstrates genuine understanding:** Acknowledge it, then extend: "Good — now what if we change this one thing...?"
7. **If the child asks a direct factual question** ("How far is the moon?"): Answer it directly, then pivot: "Now that you know it's 384,000 km, how long do you think it would take to drive there?"

**This is fundamentally a prompt engineering and system design problem, not a model training problem.** The dialogue manager constructs prompts that instruct the LLM to behave Socratically, informed by the learner model. The LLM's role is to generate natural, contextually appropriate questions and responses — the pedagogical strategy is encoded in the system prompts and the learner model's state.

**4.6.3 Comprehension Verification**

The Primer must distinguish genuine understanding from parroting. Techniques:

- **Transfer questions:** If the child explains concept A, ask them to apply it to a new situation B they haven't seen before.
- **Explanation requests:** "Can you explain that to me as if I were a younger child who doesn't know anything about this?"
- **Contradiction probing:** Deliberately state something slightly wrong and see if the child catches it. "So if X is true, then Y must also be true, right?" (when Y is actually false).
- **Analogy generation:** "Can you think of something else that works the same way?"

**4.6.4 Curriculum Integration**

The device does not follow a rigid curriculum. It follows the child's curiosity — but it keeps track of where the child is relative to age-appropriate knowledge benchmarks, and it gently steers exploration toward gaps.

Example: A child fascinated by dinosaurs will learn biology, geology, timescales, extinction events, evolution — all through the lens of their interest. The Primer tracks that this child is building knowledge in biology and earth sciences but hasn't yet encountered chemistry. It introduces chemical concepts through a question the child's existing knowledge makes natural: "You know how fossils form? What do you think happens to the *molecules* in a bone over millions of years?"

### 4.7 Safety and Privacy

**Absolute principles:**
- **All processing is local by default.** No conversation data leaves the device without explicit parental consent.
- **No surveillance.** The camera, when present, activates only on explicit request and processes locally.
- **No manipulation.** The device does not use dark patterns, gamification tricks, or engagement maximisation. It does not try to keep the child using it. It is happy to say "That's enough for today."
- **No advertising.** Ever.
- **Content safety.** The local model is fine-tuned/prompted to refuse inappropriate content. When connected, cloud API safety filters provide an additional layer.
- **Parental dashboard.** Parents can review (not real-time monitor) learning progress, topics covered, and time spent. They cannot read conversation transcripts — the child's intellectual exploration is private, like a diary.

**Data retention:**
- Conversation history: stored locally, encrypted at rest, auto-pruned after configurable period (default 90 days for context, learner model retains understanding state indefinitely)
- Learner model: local only, backed up to parent-controlled storage when connected
- No telemetry, no analytics, no data collection beyond what is needed for the device to function

---

## 5. Cloud Integration

When WiFi is available, the device can offload to a more capable cloud model:

**Preferred:** Anthropic Claude API (Sonnet for routine interaction, Opus for complex reasoning)
- Superior Socratic dialogue quality compared to smaller local models
- Streaming response for low-latency interaction
- The pedagogical engine (learner model, dialogue manager) runs locally regardless — the cloud model is the reasoning engine, not the pedagogical one.

**Hybrid strategy:**
1. Child speaks → Whisper (local) transcribes
2. Pedagogical engine constructs prompt (local) using learner model state
3. If connected: send prompt to cloud API, stream response
4. If offline: send prompt to local LLM (RK1828), generate response
5. Response → Piper TTS (local) → speaker

The child should not notice the difference. The transition between cloud and local should be seamless — same voice, same conversational style, same pedagogical approach. The only difference is reasoning depth.

---

## 6. Development Roadmap

### Phase 0 — Proof of Concept (Months 1–3)

**Goal:** Validate that the core interaction loop works and feels right.

**Hardware:** ROCK 5T 32GB + NVMe SSD. No custom enclosure — bare board on a desk. USB microphone + speaker. Standard monitor for text output (E Ink display not yet integrated).

**Software:**
- Install Armbian, RKNN-LLM toolkit
- Run Qwen3 7B on RK1828 accelerator (or on CPU if accelerator not yet available)
- Implement basic Socratic dialogue prompt template
- Integrate Whisper for STT, Piper for TTS
- Test with actual children (grandchildren!) in supervised sessions
- Record observations: What works? Where does the dialogue break down? What surprises us?

**Phone-based parallel test (added May 2026):** Run the Primer software stack on a Redmagic Pro (24GB RAM, Snapdragon 8 Elite or equivalent) using Ollama or llama.cpp with Gemma 4 26B A4B (Q4, ~14–16GB weights). This tests a critical strategic question: if a consumer phone can deliver acceptable Socratic dialogue latency, the Primer could become a software product distributed via app stores rather than a custom hardware device — dramatically simplifying the path to "for humanity at large." The Redmagic arrives ~mid-May 2026; initial benchmarks (tok/s, thermal throttling under sustained inference, battery drain) should be recorded and compared against the SBC results.

**Key questions to answer:**
- Is the 7B or MoE 26B model good enough for Socratic dialogue with a child, or do we need the cloud model for the pedagogical quality we're after?
- Can a phone-class device sustain conversational inference without thermal throttling, or does the Primer fundamentally need a purpose-built device with better thermal management?
- Does the Gemma 4 26B A4B MoE architecture deliver meaningfully better Socratic reasoning than a dense 7B model, justifying the higher RAM requirement?

### Phase 1 — Pedagogical Engine (Months 3–9)

**Goal:** Build the learner model and dialogue manager.

**Software focus:**
- Implement concept graph and understanding-depth tracking
- Develop Socratic dialogue manager with comprehension verification
- Build RAG pipeline: Wikipedia + curated knowledge base + FTS5
- Develop system prompts that reliably produce Socratic behaviour across conversation contexts
- Iterate with child testers — this is the phase where the pedagogical design matters most

**Hardware:** Same as Phase 0, plus E Ink display integration (begin custom UI development).

**Key question:** How do we represent a child's understanding in a way that is accurate enough to guide the dialogue manager but light enough to maintain on an edge device?

### Phase 2 — Integration and Form Factor (Months 9–15)

**Goal:** Assemble the complete device in a form factor children can use.

**Hardware:**
- Custom PCB or carrier board integrating RK3588 module + RK1828 (or CIX P1 module standalone)
- E Ink display with capacitive touch
- MEMS microphone array + speaker
- Battery management system
- 3D-printed enclosure (iterating toward injection mould design)
- Camera module integration

**Software:**
- Complete offline knowledge base assembly
- Cloud/local hybrid switching
- Parental dashboard (web-based, accessed from phone/computer)
- OTA update mechanism for knowledge base and model updates

### Phase 3 — Field Testing (Months 15–24)

**Goal:** Extended testing with children in real-world conditions.

**Deployment:**
- 5–10 units with families (starting with grandchildren, expanding to friends/colleagues)
- Diverse age range (5–14)
- Mix of urban/rural, connected/intermittent connectivity
- Structured observation and feedback collection

**Metrics:**
- Engagement: session frequency and length, voluntary use
- Learning: concept acquisition rate compared to age norms
- Satisfaction: child and parent qualitative feedback
- Reliability: uptime, battery life, thermal performance, drop survival

### Phase 4 — Open Source and Community (Months 24+)

**Goal:** Release the project as open-source for community development.

- Hardware design files (KiCad) published
- Software stack fully open (Apache 2.0 or similar)
- Knowledge base curation tools released
- Curriculum authoring tools for educators
- Community forum and contribution guidelines
- Localisation framework for non-English languages

---

## 7. Bill of Materials (Prototype, Estimated)

**Option A: RK3588-based prototype**

| Component | Estimated Cost (AUD) |
|---|---|
| Radxa ROCK 5T 32GB | $350 |
| Rockchip RK1828 M.2 module | $80–120 (est.) |
| 256GB NVMe SSD | $50 |
| 7.8" colour E Ink display + touch | $150–250 |
| MEMS microphone array | $30 |
| Speaker + Class-D amplifier | $20 |
| 100Wh LiPo battery + BMS | $80 |
| USB-C PD charging circuit | $15 |
| 3D-printed enclosure | $30 |
| Misc (cables, connectors, thermal) | $30 |
| **Total (prototype)** | **$835–$975 AUD** |

**Option B: Orange Pi 6 Plus prototype (added May 2026)**

| Component | Estimated Cost (AUD) |
|---|---|
| Orange Pi 6 Plus 64GB | ~$2000 |
| ~~RK1828 module~~ | ~~not needed~~ |
| 256GB NVMe SSD | $50 |
| 7.8" colour E Ink display + touch | $150–250 |
| MEMS microphone array | $30 |
| Speaker + Class-D amplifier | $20 |
| 100Wh LiPo battery + BMS | $80 |
| USB-C PD charging circuit | $15 |
| 3D-printed enclosure | $30 |
| Misc (cables, connectors, thermal) | $30 |
| **Total (prototype)** | **~$2,405–$2,505 AUD** |

The CIX P1 option is expensive now but simpler (no separate accelerator). At current ARM SBC pricing trajectories, equivalent boards should reach $200–300 within 2–3 years, at which point Option B becomes cheaper than Option A due to the eliminated RK1828.

At scale, with custom PCB and injection moulding, the target BOM is $400–500 AUD (either platform, at maturity pricing).

---

## 8. Open Research Questions

These are the hard problems — the ones where the three of us bring complementary expertise:

**Pedagogical (Horst + Son-in-law):**
- How do we calibrate Socratic pressure? Too much and the child disengages; too little and they don't learn to push through difficulty.
- How do we handle the gap between what a child *can* understand and what they're *ready* to understand emotionally? (A 10-year-old can grasp the concept of death; that doesn't mean every conversation should go there.)
- How do we measure understanding rather than recall? This is the fundamental assessment problem in education, and we're trying to solve it in real-time conversation.

**Machine Learning (Son-in-law + Son):**
- What is the minimum model size that produces acceptable Socratic dialogue quality? MoE architectures (e.g. Gemma 4 26B A4B with 3.8B active parameters) may dramatically lower the hardware floor for this. This determines whether the device can work fully offline or needs cloud connectivity for the core pedagogical function.
- Does a MoE model with 3.8B active parameters deliver meaningfully better Socratic reasoning than a dense 7B model? If so, the RAM/bandwidth trade-off is justified.
- Can we fine-tune a small model specifically for Socratic interaction with children, potentially outperforming a larger general model on this narrow task?
- How should the learner model be represented? Knowledge graph? Bayesian network? Something simpler? The representation must be updatable in real-time during conversation and queryable by the dialogue manager.

**Hardware (Son-in-law):**
- Thermal management in a sealed enclosure with no fan, running sustained inference at 15–25W (higher end if using CIX P1). What's the thermal budget? Do we need throttling strategies?
- The RK1828 module's availability and integration with the RK3588 host — any undocumented limitations in the M.2 interface? (Moot if CIX P1 path is chosen.)
- CIX P1 NPU runtime maturity: is the toolchain production-ready, or are we signing up for early-adopter pain?
- Battery chemistry and cycle-life trade-offs for a device expected to last 3–5 years of daily charging.
- Phone-as-Primer viability: does the Redmagic Pro 24GB sustain conversational inference (Gemma 4 26B A4B) without thermal throttling? If yes, does the Primer become a software product rather than custom hardware?
- E Ink display refresh rate and partial update strategies for a conversational UI (typing indicators, streaming text, illustrations).

**System Integration (All):**
- Latency budget: from child finishing a sentence to first audio response, what's acceptable? (Our target: < 1.5 seconds offline, < 1 second with cloud.)
- How do we handle the child growing up? The device needs to serve a 5-year-old and a 14-year-old. The interface, the voice, the complexity of dialogue, the topics — all must scale.
- Multilingual support: can we serve children who are learning a second language, switching between languages naturally?

---

## 9. What Makes This Different From a Chatbot

A chatbot answers questions. The Primer asks them.

A chatbot optimises for engagement. The Primer optimises for understanding.

A chatbot is a product. The Primer is a relationship — a patient, persistent, endlessly curious companion that cares more about whether a child can explain *why* than whether they can say *what*.

The technical challenge is substantial. The pedagogical challenge is harder. The ethical challenge — building a device that shapes how a child thinks without becoming the kind of thing that *tells* them what to think — is the hardest of all. It is also, as Daniel Osei might say, the most important question any of us have ever asked.

---

*This document is a living specification. Version 0.1 — intended to start a conversation, not end one.*
