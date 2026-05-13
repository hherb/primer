//! `Responder` impl that drives a shared `Arc<Mutex<DialogueManager>>`.
//!
//! The CLI's responder borrows `&mut DialogueManager` directly; the GUI
//! holds the DM behind an `Arc<Mutex<…>>` so other Tauri commands can
//! read its state while voice mode is active. This responder locks the
//! DM for the duration of one `respond_to_streaming` call — same blocking
//! semantic as text mode's `send_message`.
//!
//! Lifetime note: `run_loop` (spawn-based) wants `Box<dyn Responder + 'static>`.
//! `ArcDmResponder` owns an `Arc` and the returned future captures the
//! Arc directly, so the responder satisfies `'static` even though the
//! `Responder::respond` trait signature carries `'a` lifetimes on its
//! input slices.

use std::sync::Arc;
use tokio::sync::Mutex;

use primer_core::error::Result;
use primer_pedagogy::DialogueManager;
use primer_speech::voice_loop::Responder;

/// `Responder` that wraps a shared dialogue manager handle and locks it
/// per turn. See module docs for the lifetime story.
pub struct ArcDmResponder {
    dm: Arc<Mutex<DialogueManager>>,
}

impl ArcDmResponder {
    pub fn new(dm: Arc<Mutex<DialogueManager>>) -> Self {
        Self { dm }
    }
}

impl Responder for ArcDmResponder {
    fn respond<'a>(
        &'a mut self,
        transcript: &'a str,
        mut on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'a>> {
        let dm_arc = Arc::clone(&self.dm);
        Box::pin(async move {
            let mut dm = dm_arc.lock().await;
            let mut full = String::new();
            dm.respond_to_streaming(transcript, |chunk| {
                full.push_str(chunk);
                on_chunk(chunk);
            })
            .await?;
            Ok(full)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GuiConfig;
    use crate::wiring::build_active_session;
    use tempfile::TempDir;

    #[tokio::test]
    async fn responder_locks_dm_and_streams_stub_response() {
        // Build a stub-backed DM via the standard wiring so this test
        // tracks the same construction path the GUI uses in production.
        let home = TempDir::new().unwrap();
        let mut cfg = GuiConfig::default();
        cfg.persistence.no_persist = true;
        let active = build_active_session(home.path(), &cfg).await.unwrap();

        let mut responder = ArcDmResponder::new(Arc::clone(&active.dialogue_manager));
        let mut chunks: Vec<String> = Vec::new();
        let chunks_box: Box<dyn FnMut(&str) + Send + '_> = Box::new(|c: &str| {
            chunks.push(c.to_string());
        });
        let full = responder
            .respond("Hello, Primer.", chunks_box)
            .await
            .expect("stub responder produces a reply");

        assert!(!full.is_empty(), "stub backend always produces some text");
        // The chunks should reconstruct the full text.
        let joined: String = chunks.concat();
        assert_eq!(joined, full);
    }
}
