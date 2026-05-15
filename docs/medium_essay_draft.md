# We're Building Neal Stephenson's Primer. It's Open Source. And We Need Your Help.

In Neal Stephenson's 1995 novel *The Diamond Age*, a device called *A Young Lady's Illustrated Primer* changes the life of a street kid named Nell. It doesn't teach her by lecturing. It teaches her by telling stories that respond to her life, asking questions that force her to think, and never once dumbing things down. It meets her where she is and walks beside her as she figures things out.

Thirty years later, we have the technology to build it. So we are.

**The Primer** is an open-source Socratic AI learning companion for children aged 5 to 14. It runs today as a desktop application and text REPL in Rust, holds real conversations with children, and is designed from the ground up to eventually run on a battery-powered handheld device with no internet required.

It's AGPL-3.0 licensed. Every line of code is public. And we need contributors --- especially for languages beyond English and German.

## Why the World Needs This

Every child deserves a patient, brilliant tutor who knows exactly what they understand and what they don't, who follows their curiosity instead of a rigid syllabus, and who never gets tired, frustrated, or distracted.

That tutor doesn't exist at scale. Private tutoring costs $40--100 an hour. Schools have one teacher for thirty children. Parents are exhausted. The children who would benefit most from individual attention --- kids in remote areas, kids whose parents work multiple jobs, kids who learn differently --- are the ones least likely to get it.

Meanwhile, the AI tools marketed at children today are either flashcard apps with a chatbot skin, engagement-maximising screen traps, or general-purpose assistants that answer every question instantly, teaching children to consume answers rather than construct understanding.

The Primer takes a fundamentally different approach.

## What Makes It Different

### It asks more questions than it answers

When a child asks "Why is the sky blue?", the Primer doesn't recite Rayleigh scattering. It asks: "What colour does the sky turn at sunset? Why do you think it changes?" Then it walks the child toward discovering the answer themselves.

This is the Socratic method, the oldest and most effective form of teaching. The child isn't a passive receiver of facts. They're an active builder of understanding.

Pure factual questions still get direct answers --- "How far is the moon?" gets "384,000 kilometres." But even then, the Primer pivots: "Now that you know that --- how long would it take to drive there?"

### It verifies comprehension, not just engagement

Most AI learning tools assume that if a child can repeat something, they've understood it. The Primer knows better.

It probes understanding through transfer questions ("Can you explain it to someone who's never heard of it?"), application challenges ("What would happen if gravity were twice as strong?"), and contradiction probing ("Someone told me plants eat soil --- what would you say to them?").

Under the hood, a comprehension classifier assesses the depth of a child's understanding for every concept they discuss --- tracking whether they're merely aware of a term, can recall a definition, truly comprehend the idea, can apply it in new contexts, or can analyse and reason about it. That assessment is never a quiz. It emerges naturally from conversation.

### It doesn't try to keep children hooked

This is perhaps the most radical design choice: **the Primer does not maximise engagement.**

If a child wants to stop, it says "That's enough for today" without guilt. It detects frustration and disengagement from response patterns and adjusts --- offering scaffolding, suggesting a topic change, or closing the session. After thirty minutes, it gently suggests a break. The child can keep going, but the nudge is there.

No streaks. No points. No "just five more minutes." No dark patterns. The Primer is happy when the child walks away and plays outside.

### All data stays on the device

The learner model --- what the child knows, how deeply they understand it, what topics sustain their curiosity --- never leaves the device without explicit parental consent. Cloud inference (when used) sends conversation turns per-request; nothing is stored server-side.

There is no telemetry. No analytics dashboard feeding an ad network. No "anonymised" data that isn't really anonymous. The child's intellectual exploration is private, like a diary.

### It's designed to work offline

The Primer is built to run on local hardware without any network dependency. While it can use cloud AI services today (and does, for development), the architecture is specifically designed so that swapping in a local model running on a $150 single-board computer is a configuration change, not a rewrite.

This matters because the children who need it most --- in remote communities, in developing countries, in homes without reliable broadband --- cannot depend on a cloud connection.

### Voice-first is a pedagogical choice

The Primer treats voice as its primary interface. A screen can show text and diagrams, but it's never required. For younger children, a screen is actively undesirable.

The research is clear: children who gesture while explaining concepts are significantly more likely to transfer learning to new problems. A voice-only device frees the child's body to move, gesture, and manipulate objects while thinking. You can't skim a conversation the way you can skim text --- conversational speech demands the kind of effortful cognitive processing that drives deep learning.

The Primer should feel like a conversation with a favourite teacher, not like an app.

## What It Can Do Today

This isn't a whitepaper or a pitch deck. The Primer is working software. Here's what it does right now:

**Socratic conversation engine.** A dialogue manager that decides, on every turn, what pedagogical move to make --- guide with a question, scaffold with an analogy, check comprehension, extend a concept, suggest a break, or close the session. The intent system adapts based on the child's engagement, understanding, and session history.

**Streaming generation** against Claude (Anthropic API) or local models via Ollama. Tokens arrive as the model produces them.

**Persistent memory across sessions.** Every conversation persists to SQLite (one database per child, deliberately separate from the knowledge base for privacy). When a session grows past the active context window, a rolling LLM-generated summary plus full-text retrieval over older turns provide long-term memory --- the Primer remembers what it discussed with this child last week.

**An engagement classifier** that detects whether a child is curious, bored, frustrated, or struggling --- running one turn behind the conversation so it never adds latency. The assessment feeds directly into the next turn's pedagogical decision.

**A concept extractor and comprehension classifier** that together track what topics were discussed, who introduced them, and how deeply the child understood each one. These assessments build up a longitudinal learner model with per-concept depth tracking on a Bloom's taxonomy scale.

**Spaced-repetition vocabulary review.** Concepts the child has previously encountered are gently surfaced back into the conversation at expanding intervals (1, 3, 7, 14, 30 days) via a Leitner-box scheduler. The Primer weaves review words in only when topically relevant --- no drilling, no quizzing.

**Hybrid retrieval over a knowledge base.** A locale-keyed corpus (56 hand-drafted CC0 passages for English, 35 Simple English Wikipedia articles, 66 German Klexikon children's-wiki articles) auto-loads on first run. The retrieval pipeline runs both lexical search (BM25) and semantic search (dense vectors via BGE-M3) in parallel and fuses them, achieving 100% recall on 91 benchmark queries in English. The German pipeline runs in parallel with its own benchmark.

**A working voice round-trip.** Behind a cargo feature flag: Silero VAD opens the microphone, Whisper transcribes the child's speech, the dialogue engine generates a response, and Piper synthesises it phrase-by-phrase so the Primer starts speaking before generation finishes. No barge-in by design --- the Primer never speaks over the child and the child never speaks over the Primer.

**A desktop GUI** built with Tauri 2 --- session picker, markdown-rendered chat bubbles with streaming, a settings modal mirroring every CLI flag, and a sidebar that surfaces all the pedagogical signals (intent, engagement, concepts, comprehension depth, vocabulary review queue) for evaluation and debugging.

**Two languages.** English and German, with locale-scoped knowledge bases, locale-aware prompt packs, and locale-specific voice models.

The entire system is a 14-crate Rust workspace. No system dependencies for the default build (SQLite is bundled, TLS uses rustls). It compiles and runs on macOS and Linux today.

## The Architecture: Why Rust, Why Traits

The codebase is built around one architectural bet: **trait-based hardware abstraction.**

The pedagogical engine doesn't know or care whether it's talking to Claude over the network, Llama 3 running locally via Ollama, or a quantised 7B model on a phone's NPU. The `InferenceBackend` trait has three implementations today (stub, cloud, Ollama); adding llama.cpp or a Rockchip NPU backend is implementing one trait, not rewriting the application.

The same pattern holds across the system. `KnowledgeBase`, `Embedder`, `SpeechToText`, `TextToSpeech`, `VoiceActivityDetector`, `LearnerStore`, `SessionStore` --- all traits. All swappable at runtime via configuration. All testable with stubs that need no network, no model, no hardware.

This isn't over-engineering. It's the only way to build a system that runs on a MacBook during development and on a $150 single-board computer in a child's hands, without maintaining two codebases.

Rust was chosen for the same reason: the target is a battery-powered device with 32GB of RAM shared between an LLM, a speech pipeline, a knowledge base, and an OS. Garbage collection pauses and memory bloat are not acceptable when a child is mid-sentence. Rust's memory model gives us the performance of C with the safety guarantees of a managed language.

## What's Next

**Local inference.** The current priority is implementing a llama.cpp backend so the Primer can run entirely offline on consumer hardware. The trait architecture means this is additive --- the pedagogical engine doesn't change.

**More languages.** The Primer's locale system is built for this. Adding a new language requires: a prompt pack (the personality and pedagogical instructions in that language), a knowledge corpus (ideally from a children's wiki like Klexikon), and voice models (Whisper and Piper both support dozens of languages). No Rust changes needed.

**Hardware prototype.** The long-term vision is a handheld device roughly the size of a hardback children's book. Rockchip RK3588 SoC with an RK1828 NPU accelerator, colour E Ink display, MEMS microphone array, 5--7 hours of battery. A physical thing a child can hold.

**Pedagogical depth.** Curriculum alignment, multi-session learning arcs, a parental dashboard (read-only, no surveillance), and collaborative mode for two children sharing a Primer.

## How You Can Help

The Primer is one person's project today, with AI pair-programming. It needs to become a community.

**If you speak a language other than English or German:** the single highest-impact contribution is a new locale. Curate a children's-wiki whitelist for your language, translate a prompt pack, test the voice pipeline with your language's Whisper and Piper models. The infrastructure is ready --- it's waiting for content.

**If you know Rust:** the codebase is clean, well-documented, and has a [CLAUDE.md](https://github.com/your-repo/primer/blob/main/CLAUDE.md) that gives a new contributor everything they need to orient. Local inference (llama.cpp backend), CI setup, and retrieval test coverage are all open work items.

**If you know ML:** the comprehension classifier, engagement detector, and concept extractor all run on prompted LLM calls today. Fine-tuned small models would be faster, cheaper, and could run on-device. The training data is already being generated and persisted --- every conversation produces labelled examples.

**If you have children:** test it. Run the desktop app, sit with your child, and tell us what works and what doesn't. The Primer's Socratic behaviour is tuned by observation, not by metrics dashboards.

**If you're a teacher or educator:** review the pedagogical engine. The prompt builder, the intent system, the comprehension verification --- these encode pedagogical theory into software. Expert review from people who teach children every day is worth more than any benchmark.

**If you care about hardware:** the Phase 3 enclosure design is wide open. Display selection, audio hardware, thermal design, battery management --- this is where the Primer becomes a physical object.

## The Bet We're Making

The bet behind the Primer is simple: **the Socratic method, delivered with infinite patience and perfect memory, personalised to each child, available to any child regardless of geography or income, is the most important application of AI we can build.**

Not a better search engine. Not a faster code generator. Not a more convincing chatbot. A patient, curious, tireless tutor that teaches children how to think by refusing to think for them.

Stephenson imagined it in 1995. The technology exists in 2026. The question is whether we'll build it as a product that extracts value from children's attention, or as a public good that gives every child the tutor they deserve.

We chose public good. The code is open. The licence is copyleft. And the door is open.

---

*The Primer is at [github.com/hherb/primer](https://github.com/hherb/primer). Star it, clone it, or open an issue. If you want to add your language, start with the [contributor docs](https://github.com/hherb/primer/docs/devel/index.md).*

*Built in Rust. Licensed AGPL-3.0. No telemetry. No ads. No data collection. Just a conversation between a child and an AI that asks more questions than it answers.*
