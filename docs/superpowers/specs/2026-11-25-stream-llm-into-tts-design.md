# Stream LLM deltas into TTS (issue #325) — design note

**Status:** proposal, not implemented. Three owner decisions, D1–D3 below, gate step 3.
**Date:** 2026-11-25

## Problem

The voice loop's LATENT_THINK step (`voice_loop/state_machine/inner.rs`) awaits the whole LLM reply before it opens a TTS session. The child therefore hears nothing for the full decode time. At 15 tok/s, an 80-token reply is about 5 s of silence. The streaming `SynthesisSession` (`push_text` is incremental, and `finalize` drains the tail) was built for this case but is never fed incrementally.

## What stays the same

- **No barge-in** (`project_no_barge_in_pedagogy`). Once the Primer speaks, the mic stays gated until the speaker has drained: `is_speaking` plus the `DrainHook`.
- **`push_text` runs on the loop task, as it does today.** The responder's `on_chunk` callback only forwards each delta over an unbounded channel. It never calls TTS. So the macOS-native "no `spawn_blocking`" analysis in the SPEAK block still applies unchanged:
  - on the CLI, the loop task is on the OS main thread;
  - on the GUI, one worker is blocked per phrase.

  While `push_text` blocks, the LLM backend's spawned reader task keeps buffering into its own unbounded channel, so no deltas are lost.
- **The end-of-turn sequence is unchanged:** `finalize`, then the flush sentinel, then the drain hook, then clear `is_speaking`, then drain stale VAD events.

## Design

1. **Phrase boundary in the state machine, not a new stateful markdown stripper.** The loop owns a `PhraseSplitter` (already `pub` in `primer-speech`). Each delta goes in, and each *complete* phrase comes out. That phrase goes through today's whole-string `strip_markdown_for_tts`, then `session.push_text(phrase)`. At the end, `flush()` the splitter and push the tail.

   Emphasis pairs almost never span a `. ! ?` boundary. When one does, the stray marker is left in place and spoken, which is today's behaviour for an unmatched marker. This avoids the incremental stripper the issue anticipated (a `ReasoningFilter`-shaped state machine). The session's own splitter then sees whole phrases and passes them straight through.

2. **SPEAK starts at the first complete phrase.** Before that phrase arrives, LATENT_THINK behaves exactly as today: VAD `SpeechStart` cancels and the turn is retried with the continuation. When the first phrase is ready, the loop does the following, in order:
   - emit `on_transcript_finalized` and replay the buffered `on_response_chunk`s;
   - `on_state_change(Speak)`;
   - set `is_speaking`;
   - open the session and push.

   From then on, the remaining deltas are forwarded to the observer live and pushed phrase by phrase. VAD events are ignored, because the mic is gated.

3. **Errors after audio has started.** `handle_llm_err` today *replaces* the buffered chunks with `FALLBACK_LINE`. Once some phrases have been spoken, it must *append* `FALLBACK_LINE` instead: push it as one more phrase and emit it as one more observer chunk. Before the first phrase, the replace behaviour stays.

4. **Cancel after audio has started** (the Stop button, `cancel_response`). Today a cancel during SPEAK is only seen after synthesis ends: `push_text` is synchronous and the SPEAK block polls nothing. Streaming keeps that per phrase: the select loop checks `cancel_response` and `external_stop` between phrases, drops the session (no `finalize`), stops pushing and goes back to LISTEN. Samples already in the speaker ringbuf still play out, because there is no "clear speaker" hook. Adding one (`LoopBackends` gets an optional `clear_speaker` that empties the ringbuf) is a separate, optional step; it is not needed for the latency win.

## Owner decisions

- **D1: the window for continuing to speak shrinks.** Today the child can resume talking for the whole decode (~5 s on device) and the Primer retries with the longer utterance. With streaming, that window closes when the first phrase is spoken, about 1–1.5 s after `SpeechEnd`.
  - A child who pauses mid-thought for longer than that now gets an answer to half a question. That is the latency/patience trade-off at the centre of the voice pedagogy.
  - **Option:** a minimum think hold, `consts::speech::MIN_THINK_HOLD_MS`. Speech starts no earlier than N ms after `SpeechEnd`, even when a phrase is ready.
  - **Recommendation:** ship with a hold of ~1200 ms and tune it on the device.
- **D2: move the quit check before the LLM call.** Today "bye" is sent to the responder, so it becomes a DM turn plus an LLM call, and only the SPEAK is skipped. With streaming, the Primer would start speaking a reply to "bye".
  - **Recommendation:** check `is_quit_phrase` right after STT. The quit utterance then records no DM turn and costs no LLM call.
- **D3: markers that span sentences** are spoken as a bare `*` (see step 1).
  - **Recommendation:** accept this. Revisit only if a real reply shows it.

## Staged plan (one PR each, TDD)

1. **Behaviour-preserving split of `inner.rs` (514 lines).**
   - Move the LATENT_THINK select into `state_machine/think.rs` and the SPEAK block into `state_machine/speak.rs`, each as one `pub(super)` function.
   - **Acceptance:** the test count is unchanged on all three arms (`voice-loop`; `voice-loop,silero,cpal,macos-native`; `…,macos-native-26`), `inner.rs` is under 300 lines, and every file is under 500.
2. **Phrase-wise SPEAK, still after completion.**
   - SPEAK pushes `strip_markdown_for_tts(phrase)` per `PhraseSplitter` phrase instead of the whole string.
   - **Acceptance:** a mock TTS records one `push_text` call per sentence. The existing markdown tests still pass, and the audio is byte-identical for a markdown-free reply.
3. **Streaming (needs D1–D3).**
   - Delta channel, SPEAK-at-first-phrase, append-fallback, between-phrase cancel.
   - **Acceptance:**
     - A mock test with a responder that emits a phrase, then parks, then resolves. The first `push_text` and the first audio happen **before** the responder future resolves.
     - A VAD cancel before the first phrase still retries.
     - A mid-stream error after one spoken phrase speaks that phrase plus `FALLBACK_LINE`.
     - A cancel between phrases stops further `push_text` calls.
     - A D2 test: a quit phrase never reaches the responder.
4. **Optional: a `clear_speaker` hook** for an immediate stop.

## Not in scope

- Streaming into the observer before SPEAK starts; the GUI already shows the "thinking" state.
- Re-planning the macOS-native CLI runtime.
