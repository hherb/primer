//! #325 stage 3: the reply streams into TTS. Tests run on paused tokio
//! time so the `MIN_THINK_HOLD` wait and the responders' sleeps
//! auto-advance without slowing the suite.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use primer_core::consts::speech::{MIN_THINK_HOLD, STREAM_TAIL_FLUSH_IDLE};
use primer_core::error::{PrimerError, Result};
use primer_core::speech::{
    Named, StreamingTextToSpeech, SynthesisEvent, SynthesisSession, VadEvent, VoiceProfile,
};
use tokio::time::Instant;

use super::mocks::{MockEvent, MockObserver, MockStreamingStt, ScriptedStreamingStt};
use crate::voice_loop::observer::{ExitReason, VoiceState};

/// Each push: the text and when (paused-clock) it arrived.
type PushLog = Arc<Mutex<Vec<(String, Instant)>>>;

/// Presses Stop from inside the given (1-based) push, where the test
/// itself cannot run: `push_text` is synchronous.
type PressAt = Arc<Mutex<Option<(usize, tokio::sync::mpsc::Sender<()>)>>>;

/// TTS that records every `push_text` and wakes the test on each one.
/// Push number `fail_at` (1-based; 0 = never) returns an error.
struct LoggingTts {
    log: PushLog,
    pushed: Arc<tokio::sync::Notify>,
    press_at: PressAt,
    fail_at: Arc<AtomicUsize>,
}
impl Named for LoggingTts {
    fn name(&self) -> &str {
        "logging-tts"
    }
}
impl StreamingTextToSpeech for LoggingTts {
    fn sample_rate(&self) -> u32 {
        22_050
    }
    fn open_session(&self, _voice: &VoiceProfile) -> Result<Box<dyn SynthesisSession>> {
        Ok(Box::new(LoggingSession {
            log: Arc::clone(&self.log),
            pushed: Arc::clone(&self.pushed),
            press_at: Arc::clone(&self.press_at),
            fail_at: Arc::clone(&self.fail_at),
        }))
    }
}
struct LoggingSession {
    log: PushLog,
    pushed: Arc<tokio::sync::Notify>,
    press_at: PressAt,
    fail_at: Arc<AtomicUsize>,
}
impl SynthesisSession for LoggingSession {
    fn push_text(&mut self, text: &str, _on_event: &mut dyn FnMut(SynthesisEvent)) -> Result<()> {
        let n = {
            let mut log = self.log.lock().unwrap();
            log.push((text.to_string(), Instant::now()));
            log.len()
        };
        self.pushed.notify_one();
        if let Some((at, tx)) = self.press_at.lock().unwrap().as_ref()
            && *at == n
        {
            tx.try_send(()).unwrap();
        }
        if self.fail_at.load(Ordering::SeqCst) == n {
            return Err(PrimerError::Speech("tts boom".into()));
        }
        Ok(())
    }
    fn finalize(self: Box<Self>, _on_event: &mut dyn FnMut(SynthesisEvent)) -> Result<()> {
        Ok(())
    }
}

/// One scripted step of a streaming reply.
#[derive(Clone)]
enum Step {
    Chunk(&'static str),
    Sleep(Duration),
    Fail,
}

/// Plays `script` on its first call and answers "Ok." on later ones.
/// Records each transcript it was called with.
struct ScriptedResponder {
    script: Vec<Step>,
    calls: Arc<Mutex<Vec<String>>>,
}
impl super::Responder for ScriptedResponder {
    fn respond<'a>(
        &'a mut self,
        transcript: &'a str,
        mut on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'a>> {
        let mut calls = self.calls.lock().unwrap();
        calls.push(transcript.to_string());
        let script = if calls.len() == 1 {
            self.script.clone()
        } else {
            vec![Step::Chunk("Ok.")]
        };
        Box::pin(async move {
            let mut full = String::new();
            for step in script {
                match step {
                    Step::Chunk(c) => {
                        on_chunk(c);
                        full.push_str(c);
                    }
                    Step::Sleep(d) => tokio::time::sleep(d).await,
                    Step::Fail => return Err(PrimerError::Inference("boom".into())),
                }
            }
            Ok(full)
        })
    }
}

struct Rig {
    log: PushLog,
    pushed: Arc<tokio::sync::Notify>,
    press_at: PressAt,
    fail_at: Arc<AtomicUsize>,
    calls: Arc<Mutex<Vec<String>>>,
    observer: MockObserver,
}

fn rig() -> Rig {
    Rig {
        log: Arc::new(Mutex::new(Vec::new())),
        pushed: Arc::new(tokio::sync::Notify::new()),
        press_at: Arc::new(Mutex::new(None)),
        fail_at: Arc::new(AtomicUsize::new(0)),
        calls: Arc::new(Mutex::new(Vec::new())),
        observer: MockObserver::new(),
    }
}

/// An `is_speaking` flag plus a drain hook that counts its runs.
fn gate() -> (Arc<AtomicBool>, Arc<AtomicUsize>, super::DrainHook) {
    let speaking = Arc::new(AtomicBool::new(false));
    let drains = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&drains);
    let hook: super::DrainHook = Box::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    });
    (speaking, drains, hook)
}

fn responder(rig: &Rig, script: Vec<Step>) -> Box<ScriptedResponder> {
    Box::new(ScriptedResponder {
        script,
        calls: Arc::clone(&rig.calls),
    })
}

fn exits(rig: &Rig) -> Vec<ExitReason> {
    rig.observer
        .events()
        .into_iter()
        .filter_map(|e| match e {
            MockEvent::Exit(r) => Some(r),
            _ => None,
        })
        .collect()
}

fn backends(
    rig: &Rig,
    stt: Arc<dyn primer_core::speech::StreamingSpeechToText>,
) -> super::LoopBackends {
    super::LoopBackends::single_locale(
        stt,
        Arc::new(LoggingTts {
            log: Arc::clone(&rig.log),
            pushed: Arc::clone(&rig.pushed),
            press_at: Arc::clone(&rig.press_at),
            fail_at: Arc::clone(&rig.fail_at),
        }),
        VoiceProfile::default(),
        primer_core::i18n::Locale::English,
    )
}

fn one_utterance() -> tokio::sync::mpsc::Receiver<VadEvent> {
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    tx.try_send(VadEvent::SpeechStart).unwrap();
    tx.try_send(VadEvent::SpeechEnd).unwrap();
    rx
}

fn pushes(rig: &Rig) -> Vec<String> {
    rig.log
        .lock()
        .unwrap()
        .iter()
        .map(|(t, _)| t.clone())
        .collect()
}

fn chunks(rig: &Rig) -> Vec<String> {
    rig.observer
        .events()
        .into_iter()
        .filter_map(|e| match e {
            MockEvent::Chunk { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

/// The first phrase is pushed while the responder is still running, but
/// no earlier than `MIN_THINK_HOLD` after the attempt began (D1).
#[tokio::test(start_paused = true)]
async fn first_phrase_is_spoken_before_the_responder_resolves() {
    let rig = rig();
    let start = Instant::now();
    let (handle, join) = super::run_loop(
        backends(&rig, Arc::new(MockStreamingStt::new("tell me"))),
        one_utterance(),
        Box::new(ScriptedResponder {
            script: vec![
                Step::Chunk("Hello there. How"),
                Step::Sleep(Duration::from_secs(10)),
                Step::Chunk(" are you?"),
            ],
            calls: Arc::clone(&rig.calls),
        }),
        Box::new(|_| {}),
        None,
        false,
        None,
        rig.observer.clone(),
    );
    rig.pushed.notified().await;
    let (first, at) = rig.log.lock().unwrap()[0].clone();
    assert_eq!(first, "Hello there.\n\n");
    assert!(at - start >= MIN_THINK_HOLD, "held for {:?}", at - start);
    assert!(
        at - start < Duration::from_secs(10),
        "spoke before the reply finished"
    );
    assert!(!join.is_finished());

    let transcripts = join.await.unwrap().unwrap();
    drop(handle);
    assert_eq!(transcripts, vec!["tell me".to_string()]);
    assert_eq!(pushes(&rig), vec!["Hello there.\n\n", "How are you?\n\n"]);
    // Observer sees every delta exactly once, in order.
    assert_eq!(chunks(&rig), vec!["Hello there. How", " are you?"]);
    assert!(
        rig.observer
            .events()
            .iter()
            .any(|e| matches!(e, MockEvent::Complete(_)))
    );
}

/// A phrase is ready but the hold has not elapsed: the child resuming
/// still cancels and retries with the continuation; nothing was spoken
/// from the abandoned attempt.
#[tokio::test(start_paused = true)]
async fn vad_cancel_during_the_hold_still_retries() {
    let rig = rig();
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let stt = Arc::new(ScriptedStreamingStt::new(["why is", "the sky blue"]));
    let script = vec![
        Step::Chunk("Good question. "),
        Step::Sleep(Duration::from_secs(10)),
    ];
    let (handle, join) = super::run_loop(
        backends(&rig, stt),
        rx,
        Box::new(ScriptedResponder {
            script,
            calls: Arc::clone(&rig.calls),
        }),
        Box::new(|_| {}),
        None,
        false,
        None,
        rig.observer.clone(),
    );
    tx.send(VadEvent::SpeechStart).await.unwrap();
    tx.send(VadEvent::SpeechEnd).await.unwrap();
    // Well inside the hold: the child keeps talking.
    tokio::time::sleep(MIN_THINK_HOLD / 2).await;
    tx.send(VadEvent::SpeechStart).await.unwrap();
    tx.send(VadEvent::SpeechEnd).await.unwrap();
    drop(tx);
    join.await.unwrap().unwrap();
    drop(handle);
    assert_eq!(
        *rig.calls.lock().unwrap(),
        vec!["why is".to_string(), "why is the sky blue".to_string()]
    );
    assert_eq!(pushes(&rig), vec!["Ok.\n\n"]);
}

/// An error after audio has started appends the fallback line instead
/// of replacing what was already spoken.
#[tokio::test(start_paused = true)]
async fn mid_stream_error_after_a_spoken_phrase_appends_the_fallback() {
    let rig = rig();
    let (_handle, join) = super::run_loop(
        backends(&rig, Arc::new(MockStreamingStt::new("tell me"))),
        one_utterance(),
        Box::new(ScriptedResponder {
            script: vec![
                Step::Chunk("First part. "),
                Step::Sleep(Duration::from_secs(10)),
                Step::Fail,
            ],
            calls: Arc::clone(&rig.calls),
        }),
        Box::new(|_| {}),
        None,
        false,
        None,
        rig.observer.clone(),
    );
    join.await.unwrap().unwrap();
    assert_eq!(
        pushes(&rig),
        vec![
            "First part.\n\n",
            "Sorry, I had trouble with that.\n\n",
            "Could you ask again?\n\n"
        ]
    );
    assert_eq!(
        chunks(&rig),
        vec![
            "First part. ",
            super::FALLBACK_SEPARATOR,
            super::FALLBACK_LINE
        ]
    );
    assert!(
        rig.observer
            .events()
            .iter()
            .any(|e| matches!(e, MockEvent::InferenceError(_)))
    );
}

/// A Stop between phrases stops further pushes, and the mic gate is
/// cleared only after the drain hook has run.
#[tokio::test(start_paused = true)]
async fn cancel_between_phrases_stops_pushing_and_drains_before_ungating() {
    let rig = rig();
    let speaking = Arc::new(AtomicBool::new(false));
    let drains = Arc::new(AtomicUsize::new(0));
    let gated_at_drain = Arc::new(AtomicBool::new(false));
    let hook: super::DrainHook = {
        let (speaking, drains, gated) = (
            Arc::clone(&speaking),
            Arc::clone(&drains),
            Arc::clone(&gated_at_drain),
        );
        Box::new(move || {
            gated.store(speaking.load(Ordering::SeqCst), Ordering::SeqCst);
            drains.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        })
    };
    let (handle, join) = super::run_loop(
        backends(&rig, Arc::new(MockStreamingStt::new("tell me"))),
        one_utterance(),
        Box::new(ScriptedResponder {
            script: vec![
                Step::Chunk("One. "),
                Step::Sleep(Duration::from_secs(10)),
                Step::Chunk("Two. "),
            ],
            calls: Arc::clone(&rig.calls),
        }),
        Box::new(|_| {}),
        Some(hook),
        false,
        Some(Arc::clone(&speaking)),
        rig.observer.clone(),
    );
    rig.pushed.notified().await;
    assert!(speaking.load(Ordering::SeqCst), "gated while speaking");
    handle.cancel_response_tx.try_send(()).unwrap();
    join.await.unwrap().unwrap();

    assert_eq!(pushes(&rig), vec!["One.\n\n"]);
    assert_eq!(drains.load(Ordering::SeqCst), 1);
    assert!(
        gated_at_drain.load(Ordering::SeqCst),
        "drain ran before un-gating"
    );
    assert!(!speaking.load(Ordering::SeqCst), "un-gated after the drain");
    assert!(rig.observer.events().iter().any(|e| matches!(
        e,
        MockEvent::StateChange { state: VoiceState::Listen, hint: Some(h) } if h == "user_cancel"
    )));
}

/// D2: a quit phrase ends the loop without an LLM call.
#[tokio::test(start_paused = true)]
async fn quit_phrase_never_reaches_the_responder() {
    let rig = rig();
    let (_handle, join) = super::run_loop(
        backends(&rig, Arc::new(MockStreamingStt::new("goodbye"))),
        one_utterance(),
        Box::new(ScriptedResponder {
            script: vec![Step::Chunk("Bye!")],
            calls: Arc::clone(&rig.calls),
        }),
        Box::new(|_| {}),
        None,
        false,
        None,
        rig.observer.clone(),
    );
    let transcripts = join.await.unwrap().unwrap();
    assert_eq!(transcripts, vec!["goodbye".to_string()]);
    assert!(rig.calls.lock().unwrap().is_empty(), "no LLM call");
    assert!(pushes(&rig).is_empty());
    // Exactly one exit, and it is the keyword one (the quit branch used to
    // fall through to the end-of-loop `UserStop` exit as well).
    let exits: Vec<_> = rig
        .observer
        .events()
        .into_iter()
        .filter_map(|e| match e {
            MockEvent::Exit(r) => Some(r),
            _ => None,
        })
        .collect();
    assert!(
        matches!(exits.as_slice(), [ExitReason::Keyword]),
        "{exits:?}"
    );
}

/// The reply's closing sentence rarely ends in whitespace, so the
/// splitter holds it until the stream closes, which is only after the
/// dialogue manager's post-reply work. It is spoken after a short pause
/// instead.
#[tokio::test(start_paused = true)]
async fn final_sentence_without_trailing_space_is_spoken_after_a_pause() {
    let rig = rig();
    let (_handle, join) = super::run_loop(
        backends(&rig, Arc::new(MockStreamingStt::new("tell me"))),
        one_utterance(),
        responder(
            &rig,
            vec![
                Step::Chunk("The sun is hot. Why?"),
                // The DM's summary refresh, after the last token.
                Step::Sleep(Duration::from_secs(10)),
            ],
        ),
        Box::new(|_| {}),
        None,
        false,
        None,
        rig.observer.clone(),
    );
    rig.pushed.notified().await;
    rig.pushed.notified().await;
    let log = rig.log.lock().unwrap().clone();
    assert_eq!(log[1].0, "Why?\n\n");
    assert!(log[1].1 - log[0].1 >= STREAM_TAIL_FLUSH_IDLE);
    assert!(log[1].1 - log[0].1 < Duration::from_secs(10), "not held");
    assert!(!join.is_finished());
    join.await.unwrap().unwrap();
    assert_eq!(pushes(&rig), vec!["The sun is hot.\n\n", "Why?\n\n"]);
}

/// An error after audio started speaks the partial sentence the child
/// already sees, then the fallback, shown on its own paragraph.
#[tokio::test(start_paused = true)]
async fn mid_stream_error_speaks_the_held_partial_before_the_fallback() {
    let rig = rig();
    let (_handle, join) = super::run_loop(
        backends(&rig, Arc::new(MockStreamingStt::new("tell me"))),
        one_utterance(),
        responder(
            &rig,
            vec![
                Step::Chunk("First part. And then"),
                Step::Sleep(Duration::from_secs(10)),
                Step::Fail,
            ],
        ),
        Box::new(|_| {}),
        None,
        false,
        None,
        rig.observer.clone(),
    );
    join.await.unwrap().unwrap();
    assert_eq!(
        pushes(&rig),
        vec![
            "First part.\n\n",
            "And then\n\n",
            "Sorry, I had trouble with that.\n\n",
            "Could you ask again?\n\n"
        ]
    );
}

/// Once the reply has resolved its remaining phrases are pushed back to
/// back with no `select!` in between; Stop is still honoured before each.
#[tokio::test(start_paused = true)]
async fn stop_after_the_reply_resolved_skips_the_remaining_phrases() {
    let rig = rig();
    let (handle, join) = super::run_loop(
        backends(&rig, Arc::new(MockStreamingStt::new("tell me"))),
        one_utterance(),
        responder(
            &rig,
            vec![
                Step::Chunk("One. "),
                Step::Sleep(Duration::from_secs(5)),
                Step::Chunk("Two. Three. Four."),
            ],
        ),
        Box::new(|_| {}),
        None,
        false,
        None,
        rig.observer.clone(),
    );
    *rig.press_at.lock().unwrap() = Some((2, handle.cancel_response_tx.clone()));
    join.await.unwrap().unwrap();
    assert_eq!(pushes(&rig), vec!["One.\n\n", "Two.\n\n"]);
    assert!(rig.observer.events().iter().any(|e| matches!(
        e,
        MockEvent::StateChange { state: VoiceState::Listen, hint: Some(h) } if h == "user_cancel"
    )));
}

/// A TTS error mid-stream still drains the speaker and un-gates the mic
/// before the loop returns the error.
#[tokio::test(start_paused = true)]
async fn tts_error_mid_stream_still_runs_the_end_of_turn_tail() {
    let rig = rig();
    rig.fail_at.store(2, Ordering::SeqCst);
    let (speaking, drains, hook) = gate();
    let (_handle, join) = super::run_loop(
        backends(&rig, Arc::new(MockStreamingStt::new("tell me"))),
        one_utterance(),
        responder(
            &rig,
            vec![
                Step::Chunk("One. Two. "),
                Step::Sleep(Duration::from_secs(10)),
            ],
        ),
        Box::new(|_| {}),
        Some(hook),
        false,
        Some(Arc::clone(&speaking)),
        rig.observer.clone(),
    );
    assert!(join.await.unwrap().is_err(), "the TTS error ends the loop");
    assert_eq!(drains.load(Ordering::SeqCst), 1);
    assert!(!speaking.load(Ordering::SeqCst), "mic un-gated");
    // #429: the UI is told the loop ended, not left frozen on SPEAK.
    assert_eq!(exits(&rig), vec![ExitReason::Error]);
    assert!(rig.observer.events().iter().any(|e| matches!(
        e,
        MockEvent::StateChange {
            state: VoiceState::Exit,
            ..
        }
    )));
}

/// #429: a TTS error in a non-streamed SPEAK (the reply resolved before
/// the hold) also reports the exit.
#[tokio::test(start_paused = true)]
async fn tts_error_in_a_whole_reply_speak_reports_the_exit() {
    let rig = rig();
    rig.fail_at.store(1, Ordering::SeqCst);
    let (_handle, join) = super::run_loop(
        backends(&rig, Arc::new(MockStreamingStt::new("tell me"))),
        one_utterance(),
        responder(&rig, vec![Step::Chunk("Hi.")]),
        Box::new(|_| {}),
        None,
        false,
        None,
        rig.observer.clone(),
    );
    assert!(join.await.unwrap().is_err());
    assert_eq!(exits(&rig), vec![ExitReason::Error]);
}

/// No barge-in: the child talking during a streamed SPEAK neither cancels
/// nor shortens the reply, and is not heard as a new turn.
#[tokio::test(start_paused = true)]
async fn child_speech_during_a_streamed_speak_is_ignored() {
    let rig = rig();
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    tx.try_send(VadEvent::SpeechStart).unwrap();
    tx.try_send(VadEvent::SpeechEnd).unwrap();
    let (_handle, join) = super::run_loop(
        backends(&rig, Arc::new(MockStreamingStt::new("tell me"))),
        rx,
        responder(
            &rig,
            vec![
                Step::Chunk("One. "),
                Step::Sleep(Duration::from_secs(5)),
                Step::Chunk("Two. "),
            ],
        ),
        Box::new(|_| {}),
        None,
        false,
        None,
        rig.observer.clone(),
    );
    rig.pushed.notified().await;
    tx.send(VadEvent::SpeechStart).await.unwrap();
    tx.send(VadEvent::SpeechEnd).await.unwrap();
    drop(tx);
    let transcripts = join.await.unwrap().unwrap();
    assert_eq!(pushes(&rig), vec!["One.\n\n", "Two.\n\n"]);
    assert_eq!(rig.calls.lock().unwrap().len(), 1);
    assert_eq!(transcripts, vec!["tell me".to_string()]);
}

/// End voice mode during a streamed SPEAK exits once, after the drain.
#[tokio::test(start_paused = true)]
async fn external_stop_during_a_streamed_speak_exits_after_the_drain() {
    let rig = rig();
    let (speaking, drains, hook) = gate();
    let (handle, join) = super::run_loop(
        backends(&rig, Arc::new(MockStreamingStt::new("tell me"))),
        one_utterance(),
        responder(
            &rig,
            vec![
                Step::Chunk("One. "),
                Step::Sleep(Duration::from_secs(10)),
                Step::Chunk("Two. "),
            ],
        ),
        Box::new(|_| {}),
        Some(hook),
        false,
        Some(Arc::clone(&speaking)),
        rig.observer.clone(),
    );
    rig.pushed.notified().await;
    handle.stop_tx.send(()).unwrap();
    let transcripts = join.await.unwrap().unwrap();
    assert_eq!(transcripts, vec!["tell me".to_string()]);
    assert_eq!(pushes(&rig), vec!["One.\n\n"]);
    assert!(matches!(exits(&rig).as_slice(), [ExitReason::UserStop]));
    assert_eq!(drains.load(Ordering::SeqCst), 1);
    assert!(!speaking.load(Ordering::SeqCst));
}

/// #430: on a truncation retry the DM streams the cut-off partial, the
/// blank-line notice separator, then the apology. They are spoken as
/// separate phrases, the unterminated partial first, while the retry is
/// still generating.
#[tokio::test(start_paused = true)]
async fn a_blank_line_ends_an_unterminated_phrase() {
    let rig = rig();
    let (_handle, join) = super::run_loop(
        backends(&rig, Arc::new(MockStreamingStt::new("tell me"))),
        one_utterance(),
        responder(
            &rig,
            vec![
                Step::Chunk("the sun is a star that"),
                Step::Chunk("\n\n"),
                Step::Chunk("Sorry, I ran out of room. "),
                Step::Sleep(Duration::from_secs(10)),
                Step::Chunk("The sun is a star."),
            ],
        ),
        Box::new(|_| {}),
        None,
        false,
        None,
        rig.observer.clone(),
    );
    rig.pushed.notified().await;
    assert_eq!(pushes(&rig)[0], "the sun is a star that\n\n");
    assert!(!join.is_finished(), "spoken while the retry generates");
    join.await.unwrap().unwrap();
    assert_eq!(
        pushes(&rig),
        vec![
            "the sun is a star that\n\n",
            "Sorry, I ran out of room.\n\n",
            "The sun is a star.\n\n"
        ]
    );
}
