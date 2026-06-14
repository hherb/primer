# The Bug Was in the Seams: Getting a Local LLM to Hold a Conversation on a Phone's NPU

*How we ran a harnessed, multi-subsystem LLM application entirely on the Hexagon NPU of a gaming phone — and why the final blocker wasn't where any of us were looking.*

---

There is a particular kind of engineering problem that doesn't yield to cleverness, only to patience. You are not writing a clever algorithm. You are standing at the boundary between four systems that were each designed by people who never spoke to each other — a Rust application, a closed-source inference SDK, a custom Android ROM, and a DSP whose memory allocator predates the phone it ships in — and you are trying to make a child's question travel all the way through and come back as a coherent answer. Every layer hides its own state behind a generic error code. The phone won't even show you its logs.

This is the story of getting **the Primer** — an open-source Socratic AI tutor for children — to hold a real multi-turn conversation entirely on the **Neural Processing Unit (NPU)** of a RedMagic 11 Pro. No cloud. No internet. The whole model, the retrieval pipeline, the engagement classifiers, all of it, running on a phone you can buy at a mall.

It eventually worked, and worked *beautifully* — near-instant, "feels like sitting on my MacBook," in the words of the project's owner. But the last bug took a full session to find, and it was hiding in exactly the place that makes edge AI hard: **the seam between two components that each behaved correctly on their own.**

## Why bother running it locally at all

The easy version of an AI tutor is a thin client that calls a frontier model over the network. It works today. So why fight a phone's NPU for weeks?

Three reasons, and they're the same three reasons that make on-device inference a real and growing discipline rather than a stunt:

1. **Privacy.** This is a product for children. "All learner data is local; cloud inference sends turns per-request only" is a design principle, not a marketing line. The strongest version of that promise is one where the device can work with the network cable unplugged — where there is no per-request anything.
2. **Cost and reach.** The children who would benefit most from a patient tutor are often the ones least likely to have a reliable data plan. A device that runs offline reaches them. A metered API does not.
3. **Latency.** A Socratic dialogue is a back-and-forth. Round-tripping every turn to a datacenter adds a tax that, multiplied across a learning session, is the difference between a conversation and a form.

So the target was always the edge. The question was whether a 4-billion-parameter model, quantized and harnessed inside a real application with a dozen moving parts, could actually run on a handheld accelerator fast enough to feel alive.

## The hardware, and the SDK that hides it

The test device is a **RedMagic 11 Pro**: a Snapdragon 8 Elite Gen 5 (internally `SM8850`), 24 GB of RAM, and a **Hexagon NPU** that Qualcomm exposes through a stack called **QAIRT** — the Qualcomm AI Runtime — and its conversational layer, **Genie**. You feed Genie a quantized model bundle and a JSON config, and it gives you a C API: create a dialog, run a query, stream tokens back through a callback.

The Primer's architecture is deliberately decoupled from any of this. The pedagogical engine talks to an `InferenceBackend` trait; whether that trait is implemented by Anthropic's cloud API, a local `llama.cpp`, or the Qualcomm NPU is a runtime config choice. The NPU implementation, `QnnBackend`, is a thin safe wrapper: it `dlopen`s `libGenie.so` at runtime, resolves the six C functions it needs by hand (a closed-source SDK behind a developer-portal login is not something you can just `bindgen` and vendor into a public AGPL repo), and bridges Genie's token callback into an async stream.

That clean abstraction is what let us build and test 95% of the system on a Mac. It is also what made the final bug so hard to see — because the bug lived in the one place the abstraction couldn't reach: the *behavior* of the real SDK on the real device.

## Act I: Getting the first token out (the bring-up)

Before a conversation, you need a single token. Getting one took clearing three blockers, each of which presented as the same useless symptom: `GenieDialog_create` returns `-1`, "general error."

The first lesson of edge work arrived immediately: **this ROM has a dead `logcat` and a black `screencap`.** The two tools you'd normally reach for to see what the device is doing simply do not function. We were debugging a black box that refused to describe itself.

So we taught it to talk. Genie has an optional logging API; we wired it to write to a file inside the app's private storage, readable from the host with `adb shell run-as <pkg> cat .primer/genie.log`. Suddenly the generic `-1` had a story behind it. Three stories, in fact:

- **The V81 stub mismatch.** The runtime detects the SM8850 as a Hexagon **V81** architecture and demands `libQnnHtpV81Stub.so`. We'd been shipping V79 libraries on the (wrong) assumption that they were backward-compatible on a newer part. Fix: stage a *coherent* set of QAIRT 2.45 V81 libraries — stub, skel, and the matching `libGenie`/`libQnnHtp` from the *same build*, so the stub↔runtime↔DSP-skeleton triple has zero version skew.
- **FastRPC's vendor library, undeclared.** Reaching the DSP goes through FastRPC, whose public vendor library is `libcdsprpc.so`. On API 31+, Android refuses to load a public native library unless the app *declares* it: `<uses-native-library android:name="libcdsprpc.so">` in the manifest. Without that line, you get `loadRemoteSymbols err 4000` — which, again, surfaces as a generic create failure several layers up.
- **The skeleton that never extracted.** The DSP skeleton library has to exist as a *real file* on disk for FastRPC to push it to the Hexagon. By default, modern Android packs native libraries *inside* the APK without extracting them. The skeleton lived only at `base.apk!/lib/...`, FastRPC had no file to load (`Failed to load skel, error 1002`), and — you guessed it — `create` returned `-1`. Fix: `jniLibs.useLegacyPackaging = true`.

None of these are intellectually deep. All of them were *invisible* until we built the log-to-file path. The recurring theme of edge debugging: **most of the work is making the failure observable; the fix is often one line.**

With those cleared, the Primer's own backend generated its first token on the Hexagon NPU. A real milestone — and, it turned out, the easy half.

## Act II: The memory wall

A token is not a conversation. The next failure was a *stable* token across a reboot, and the culprit was **contiguous DSP memory**.

The model bundle ships as four weight-shared "context binaries," and Genie maps each onto the DSP. The fourth binary needed a roughly **698 MB** NSP buffer. The device's **CMA** (Contiguous Memory Allocator) free pool was about **637 MB even immediately after a reboot**, and settled closer to 374 MB in normal use. The buffer didn't fit. On stock, unrooted hardware — which is the whole point, because the real product is unrooted kids' devices — you cannot simply grow CMA; it's a kernel boot parameter.

The instinct is to shrink the runtime context size in the config. It doesn't help, and *why* it doesn't help is the interesting part: **Genie initializes every graph baked into the binary, regardless of the runtime `size` you request.** The model had been exported with a *list* of context lengths — `[512, 1024, 2048, 3072, 4096]` — and all five graphs were compiled into those weight-shared binaries. The big ones (`cl3072`, `cl4096`) were what drove the 698 MB buffer, and they loaded whether you asked for them or not.

The fix was to **re-export the model with a single context length, `--context-length 2048`**. One value, so the large graphs are never baked in, so the NSP buffers shrink to something CMA can actually hand out. (A subtle trap: the `.bin` *weights* are context-independent and barely change size between exports — if you judge the fix by file size, you'll think it did nothing. The buffers that matter are allocated at *load*, and they scale with context.)

That export took 48 hours over a throttled link from a remote location, which is its own kind of edge constraint. But when it landed, all four context binaries loaded, all eight graphs executed, and a real templated turn streamed a coherent reply on the DSP. The memory wall was down.

## Act III: The context mystery, and the bug in the seams

Here is where it got genuinely hard, and where the lesson lives.

With the 2K-context model loaded, the very first conversation turn worked — coherent, fast. And then the second prompt did *nothing*. The log showed the same line, over and over:

```
Context limit exceeded (1938 + 655 > 2048)
Context limit exceeded (1938 + 1088 > 2048)
Context limit exceeded (1938 + 176 > 2048)
```

The obvious reading: the prompt is too big. A Socratic system prompt, plus three retrieved knowledge passages, plus the question, was overflowing the 2048-token window and leaving no room to generate a reply.

So we did the obvious thing, and it was *good engineering applied to the wrong problem*. We built a clean, pure, well-tested token-budget module: estimate tokens, truncate knowledge passages to their relevant lead at a sentence boundary, assemble the system prompt under a hard ceiling while never trimming the pedagogically-load-bearing Socratic base. We tightened the conversation window. We added graceful handling so a context-limit return completed the turn with whatever had streamed instead of dropping it. We added a templated smoke-check so startup stopped running the model to context-full. All of it correct. All of it shipped.

Then we rebuilt, measured on-device, and the prompt token count went *up*. 1938, where it had been 1814.

That number — **constant at 1938 across four different generations** — was the clue, if you knew how to read it. A prompt that's too big grows when you add conversation history. This one didn't grow. It was *pinned*. And a quantity that's pinned near the maximum, identical across queries of wildly different generation lengths, is not a prompt. **It's accumulated state.**

The breakthrough came from the human in the loop. The project owner, watching the behavior, said: *"My suspicion: we have background workers for sentiment analysis etc. using inference, not just the dialog. That might fill up the context window too."*

That was it. The Primer doesn't make one LLM call per turn. It makes *four*: the chat response, plus three background subsystems — an engagement classifier, a concept extractor, and a comprehension assessor — that each run an inference call to analyze the exchange. And by configuration, all three **shared the same backend instance**, which meant they shared the **same Genie dialog handle**.

A Genie dialog is *stateful*. It maintains a KV cache across queries — that's what makes it a "dialog." Every `GenieDialog_query` *appends* to that cache. But the Primer's design is *stateless*: it builds the entire prompt — system instructions, retrieved knowledge, full conversation history — fresh on every single call, and sends the whole thing each time. The two models are fundamentally incompatible. We were re-sending the complete conversation each turn *while Genie was also retaining it*, and then doing it three more times per turn with the background workers, all into the same accumulating context. Within a turn or two, the 2048-token window was full of redundant, duplicated history. `1938` wasn't a prompt. It was a saturated cache that had nowhere left to grow.

The fix was almost insultingly small. We dumped the symbol table of the on-device `libGenie.so` with `llvm-nm` — a trick worth far more than a verbose-log rebuild for the question "does this function exist?" — and there it was:

```
GenieDialog_reset
```

QAIRT 2.45 exports a function to reset a dialog to its initial state. We bound it, added a `reset()` method to our dialog trait, and called it **before every query**. Because every inference path — chat and all three subsystems — routes through the same `generate_stream`, one reset call covers all four. Each query now starts from an empty context, which is exactly what a stateless-prompt engine needs.

We rebuilt. The owner ran a three-turn conversation. The log was clean — *zero* context-limit lines. The replies came back faster than he could read them.

## What the seams teach you

The prompt-budget work wasn't wasted — a 2K context is genuinely tight, and trimming gives real headroom. But it wasn't the bug. The bug was in the **contract mismatch between two components that were each behaving exactly as designed.** Genie's dialog was correctly stateful. Our engine was correctly stateless. Neither was wrong. The defect lived in the assumption, never written down anywhere, that one of them knew what the other was doing.

That is the recurring shape of edge-AI engineering, and the lessons generalize well beyond Qualcomm:

- **The bug is usually in the seam, not the component.** Each layer had been unit-tested in isolation and passed. The failure only existed in the interaction — a shared, stateful resource used by a stateless caller, multiplied by background workers nobody was thinking about as "inference."
- **Make the black box observable before you try to fix it.** Half this project was building diagnostics: log-to-file when `logcat` is dead, `llvm-nm` to confirm a symbol exists, `sqlite3` on a pulled database to count turns, and — most valuable of all — *reasoning from the shape of a number*. A constant 1938 told us more than any stack trace.
- **A constant is a different bug than a growing one.** If that token count had grown with each turn, it would have been prompt size, and our budget work would have fixed it. It was *flat*, which meant saturated state. Learn to read the difference; it routes you to the right hypothesis in seconds instead of hours.
- **The host/mock test boundary is what keeps you sane.** Because `QnnBackend` is generic over a `GenieDialog` trait, we could write the `reset()` logic, mock the dialog, and assert that a reset event is recorded immediately before every query — all on a Mac, before ever touching the device. The on-device run *confirmed* the fix; it didn't *discover* it.
- **Keep a human in the loop.** The decisive insight — "the background workers share the dialog" — came from a person who understood the *system*, not just the *symptom*. The machine was excellent at building the budget module, running the experiments, dumping the symbols. The human knew which question to ask.

Edge inference is often framed as a problem of raw performance — quantization, kernels, tok/s. Those matter. But the harder, less-discussed half is *systems integration under hostile observability*: a closed SDK, a ROM that hides its logs, a memory allocator you can't grow, and a dozen application subsystems all funneling through one accelerator that maintains hidden state. The model running fast is necessary. Getting a child's question cleanly through every seam is the actual work.

It runs now. A four-billion-parameter model, harnessed inside a real pedagogical application with retrieval and three analysis subsystems, holding a patient Socratic conversation entirely on a phone's NPU, offline, fast enough to feel like a laptop. The last bug was one missing `reset()` call. They usually are.

---

*The Primer is open-source (AGPL-3.0). The inference backend, the Genie FFI scaffold, and the prompt-budget module described here are all in the public repository.*
