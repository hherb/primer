//! #325 stage 3: the reply streams into TTS. Tests run on paused tokio
//! time so the `MIN_THINK_HOLD` wait and the responders' sleeps
//! auto-advance without slowing the suite.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use primer_core::consts::speech::MIN_THINK_HOLD;
use primer_core::error::{PrimerError, Result};
use primer_core::speech::{
    Named, StreamingTextToSpeech, SynthesisEvent, SynthesisSession, VadEvent, VoiceProfile,
};
use tokio::time::Instant;

use super::mocks::{MockEvent, MockObserver, MockStreamingStt, ScriptedStreamingStt};
use crate::voice_loop::observer::{ExitReason, VoiceState};

/// Each push: the text and when (paused-clock) it arrived.
type PushLog = Arc<Mutex<Vec<(String, Instant)>>>;

/// TTS that records every `push_text` and wakes the test on each one.
struct LoggingTts {
    log: PushLog,
    pushed: Arc<tokio::sync::Notify>,
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
        }))
    }
}
struct LoggingSession {
    log: PushLog,
    pushed: Arc<tokio::sync::Notify>,
}
impl SynthesisSession for LoggingSession {
    fn push_text(&mut self, text: &str, _on_event: &mut dyn FnMut(SynthesisEvent)) -> Result<()> {
        self.log
            .lock()
            .unwrap()
            .push((text.to_string(), Instant::now()));
        self.pushed.notify_one();
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
    calls: Arc<Mutex<Vec<String>>>,
    observer: MockObserver,
}

fn rig() -> Rig {
    Rig {
        log: Arc::new(Mutex::new(Vec::new())),
        pushed: Arc::new(tokio::sync::Notify::new()),
        calls: Arc::new(Mutex::new(Vec::new())),
        observer: MockObserver::new(),
    }
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
    assert_eq!(first, "Hello there. ");
    assert!(at - start >= MIN_THINK_HOLD, "held for {:?}", at - start);
    assert!(
        at - start < Duration::from_secs(10),
        "spoke before the reply finished"
    );
    assert!(!join.is_finished());

    let transcripts = join.await.unwrap().unwrap();
    drop(handle);
    assert_eq!(transcripts, vec!["tell me".to_string()]);
    assert_eq!(pushes(&rig), vec!["Hello there. ", "How are you? "]);
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
    assert_eq!(pushes(&rig), vec!["Ok. "]);
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
            "First part. ",
            "Sorry, I had trouble with that. ",
            "Could you ask again? "
        ]
    );
    assert_eq!(chunks(&rig), vec!["First part. ", super::FALLBACK_LINE]);
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

    assert_eq!(pushes(&rig), vec!["One. "]);
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
