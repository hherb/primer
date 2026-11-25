# Stream LLM deltas into TTS (issue #325) — design note

**Status:** stages 1–3 implemented (#425, #426, #427); stage 4 (optional `clear_speaker`) not started. Owner decisions (2026-11-25): D1 hold of 1200 ms (`MIN_THINK_HOLD`, to tune on device), D2 quit check before the LLM, D3 accept, D4 option A.
**Date:** 2026-11-25

## Problem

The voice loop's LATENT_THINK step (`voice_loop/state_machine/inner.rs`) awaits the whole LLM reply before it opens a TTS session. The child therefore hears nothing for the full decode time. At 15 tok/s, an 80-token reply is about 5 s of silence. The streaming `SynthesisSession` (`push_text` is incremental, and `finalize` drains the tail) was built for this case but is never fed incrementally.

## What stays the same

- **No barge-in** (`project_no_barge_in_pedagogy`). Once the Primer speaks, the mic stays gated until the speaker has drained: `is_speaking` plus the `DrainHook`.
- **`push_text` runs on the loop task, as it does today.** The responder's `on_chunk` callback only forwards each delta over an unbounded channel. It never calls TTS. So the macOS-native "no `spawn_blocking`" analysis in the SPEAK block still applies unchanged:
  - on the CLI, the loop task is on the OS main thread;
  - on the GUI, one worker is blocked per phrase.

  While `push_text` blocks, no deltas are lost. On a multi-threaded runtime, the LLM backend's spawned reader task keeps buffering into its own unbounded channel. On the macOS-native CLI, the runtime is current-thread, so the reader cannot run while the main thread is inside `push_text`; the bytes wait in the socket. Decode and synthesis therefore alternate on that build rather than overlap. The first-phrase latency win still holds there.
- **The end-of-turn sequence is unchanged:** `finalize`, then the flush sentinel, then the drain hook, then clear `is_speaking`, then drain stale VAD events.

## Design

1. **Phrase boundary in the state machine, not a new stateful markdown stripper.** The loop owns a `PhraseSplitter` (already `pub` in `primer-speech`). Each delta goes in, and each *complete* phrase comes out. That phrase goes through today's whole-string `strip_markdown_for_tts`, then `session.push_text(phrase + " ")`. At the end, `flush()` the splitter and push the tail.

   Emphasis pairs almost never span a `. ! ?` boundary. When one does, the stray marker is left in place and spoken, which is today's behaviour for an unmatched marker. This avoids the incremental stripper the issue anticipated (a `ReasoningFilter`-shaped state machine).

   **The trailing space is load-bearing.** Piper, Supertonic, macOS-native and `StubTts` each run `push_text` through their own `PhraseSplitter`. `find_boundary` only accepts a terminator that has whitespace after it, and the loop's splitter emits *trimmed* phrases. Without the space, `push_text("Hello.")` is held, the next push makes the buffer `"Hello.How are you?"` (no boundary), and every phrase merges and is synthesised only at `finalize`. That means zero latency win, and "Hello.How" is spoken run together. With the space, the session's splitter completes each pushed phrase at once.

   **Per attempt:** the delta channel and the loop's splitter are created fresh for each LLM attempt, just as `chunk_buffer` is cleared today on every attempt and on both cancel paths. Otherwise a partial sentence from a VAD-cancelled attempt would prefix the retry's first phrase.

2. **SPEAK starts at the first complete phrase.** Before that phrase arrives, LATENT_THINK behaves exactly as today: VAD `SpeechStart` cancels and the turn is retried with the continuation. When the first phrase is ready, the loop does the following, in order:
   - emit `on_transcript_finalized` and replay the buffered `on_response_chunk`s;
   - `on_state_change(Speak)`;
   - set `is_speaking`;
   - open the session and push.

   From then on, the remaining deltas are forwarded to the observer live and pushed phrase by phrase. VAD events are ignored, because the mic is gated.

3. **Errors after audio has started.** `handle_llm_err` today *replaces* the buffered chunks with `FALLBACK_LINE`. Once some phrases have been spoken, it must *append* `FALLBACK_LINE` instead: push it as one more phrase and emit it as one more observer chunk. Before the first phrase, the replace behaviour stays. The spoken prefix is not in the session history (see D4).

4. **Cancel after audio has started** (the Stop button, `cancel_response`). Today nothing reads `cancel_response` during SPEAK or LISTEN. It is polled only in the LATENT_THINK selects, so a press during SPEAK or LISTEN stays queued and cancels the child's *next* question (#423, a bug that predates this design). Streaming adds a check between phrases: the select loop checks `cancel_response` and `external_stop` between phrases, drops the session (no `finalize`), and stops pushing. That consumes SPEAK-time presses. LISTEN-time presses still need the #423 fix.

   **Every exit from a started SPEAK runs the end-of-turn tail.** A cancel, an `external_stop`, an error and a normal finish all do the same: flush sentinel, then drain hook, then clear `is_speaking`, then drain stale VAD events. Only then does the loop go back to LISTEN (or exit). `is_speaking` is already true by then, so a bare `continue 'outer` would leave the mic gated forever. Clearing it before the drain would let the speaker tail be transcribed as the next child turn, the self-conversation bug the gate exists to prevent.

   Samples already in the speaker ringbuf still play out, because there is no "clear speaker" hook. Adding one (`LoopBackends` gets an optional `clear_speaker` that empties the ringbuf) is a separate, optional step; it is not needed for the latency win.

## Owner decisions

- **D1: the window for continuing to speak shrinks.** Today the child can resume talking for the whole decode (~5 s on device) and the Primer retries with the longer utterance. With streaming, that window closes when the first phrase is spoken, about 1–1.5 s after `SpeechEnd`.
  - A child who pauses mid-thought for longer than that now gets an answer to half a question. That is the latency/patience trade-off at the centre of the voice pedagogy.
  - **Option:** a minimum think hold, `consts::speech::MIN_THINK_HOLD_MS`. Speech starts no earlier than N ms after `SpeechEnd`, even when a phrase is ready.
  - **Recommendation:** ship with a hold of ~1200 ms and tune it on the device.
- **D2: move the quit check before the LLM call.** Today "bye" is sent to the responder, so it becomes a DM turn plus an LLM call. The reply chunks are even replayed to the observer before the quit check, so the GUI already *shows* a reply to "bye"; only the SPEAK is skipped. With streaming, the Primer would start speaking a reply to "bye".
  - **Recommendation:** check `is_quit_phrase` right after STT. The quit utterance then records no DM turn and costs no LLM call.
- **D3: markers that span sentences** are spoken as a bare `*` (see step 1).
  - **Recommendation:** accept this. Revisit only if a real reply shows it.
- **D4: a reply cut off after audio has started is spoken but not recorded.** On a mid-stream error, the dialogue manager drops the partial Primer turn, and dropping `llm_fut` on a cancel does the same. Today that is consistent: the child hears only `FALLBACK_LINE`, never content the session lacks. With streaming, the child may have heard N phrases that the next turn's LLM context does not contain, so a child replying to them gets an answer that ignores them.
  - **Option A:** accept it. The window is a mid-stream error or a Stop press, both rare.
  - **Option B:** record the spoken prefix as the Primer turn. This needs a new `DialogueManager` path for "partial turn, spoken", which changes the dropped-partial invariant.
  - **Recommendation:** A for stage 3. Revisit B if on-device logs show it happens often.

## Staged plan (one PR each, TDD)

1. **Behaviour-preserving split of `inner.rs` (515 lines on `main`, 514 after #420).**
   - Move the LATENT_THINK select into `state_machine/think.rs` and the SPEAK block into `state_machine/speak.rs`, each as one `pub(super)` function.
   - **Acceptance:** the test count is unchanged on all three arms (`voice-loop`; `voice-loop,silero,cpal,macos-native`; `…,macos-native-26`), `inner.rs` is under 300 lines, and every file is under 500.
2. **Phrase-wise SPEAK, still after completion.**
   - SPEAK pushes `strip_markdown_for_tts(phrase) + " "` per `PhraseSplitter` phrase instead of the whole string.
   - **Acceptance:**
     - A mock TTS records one `push_text` call per sentence.
     - A test through `StubTts`, which has its own splitter, shows each pushed phrase completing before the next push. `MockStreamingTts` has no splitter, so it cannot catch a missing trailing space.
     - The existing markdown tests still pass, and the audio is byte-identical for a markdown-free reply.
3. **Streaming (needs D1–D4).**
   - Delta channel, SPEAK-at-first-phrase, append-fallback, between-phrase cancel.
   - **Acceptance:**
     - A mock test with a responder that emits a phrase, then parks, then resolves. The first `push_text` and the first audio happen **before** the responder future resolves.
     - A VAD cancel before the first phrase still retries.
     - A mid-stream error after one spoken phrase speaks that phrase plus `FALLBACK_LINE`.
     - A cancel between phrases stops further `push_text` calls, and `is_speaking` is cleared only after the drain hook has run.
     - A D2 test: a quit phrase never reaches the responder.
4. **Optional: a `clear_speaker` hook** for an immediate stop.

## Not in scope

- Streaming into the observer before SPEAK starts; the GUI already shows the "thinking" state.
- Re-planning the macOS-native CLI runtime.
