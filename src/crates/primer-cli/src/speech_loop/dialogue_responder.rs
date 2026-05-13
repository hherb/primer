//! [`Responder`] adapter that lets `&mut DialogueManager` satisfy the
//! trait. The lifetime parameter is the borrow of the manager, not of
//! its collaborators — the manager itself is `'static` now that
//! inference and knowledge are held as `Arc<dyn …>`.
//!
//! Kept in `primer-cli` (not lifted to `primer-speech`) because it
//! depends on `primer-pedagogy::DialogueManager`, a CLI/engine concern.
//! The GUI will build its own equivalent adapter in a later PR.

use primer_core::error::Result;
use primer_speech::voice_loop::Responder;

/// Wraps a mutable reference to a `DialogueManager` and implements the
/// `Responder` trait so `run_loop_borrowed` can call it without knowing
/// about the dialogue engine.
pub struct DialogueResponder<'b> {
    pub(super) dialogue: &'b mut primer_pedagogy::DialogueManager,
}

impl<'b> Responder for DialogueResponder<'b> {
    fn respond<'r>(
        &'r mut self,
        transcript: &'r str,
        mut on_chunk: Box<dyn FnMut(&str) + Send + 'r>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'r>> {
        // Own a copy of the transcript inside the future so the borrow
        // on `transcript` doesn't have to outlive the await.
        let transcript = transcript.to_string();
        Box::pin(async move {
            self.dialogue
                .respond_to_streaming(&transcript, |chunk| on_chunk(chunk))
                .await
        })
    }
}
