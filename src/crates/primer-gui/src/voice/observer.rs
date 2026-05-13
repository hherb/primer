//! `LoopObserver` impl that emits `primer://voice/*` Tauri events.

use serde::Serialize;
use tauri::Emitter;

use primer_speech::voice_loop::{ExitReason, LoopObserver, TurnCompletePayload, VoiceState};

#[derive(Serialize, Clone)]
pub struct StateChangeEvent {
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct TranscriptEvent {
    pub text: String,
}

#[derive(Serialize, Clone)]
pub struct ResponseChunkEvent {
    pub primer_turn_index: usize,
    pub text: String,
}

#[derive(Serialize, Clone)]
pub struct ResponseCompleteEvent {
    pub session_id: uuid::Uuid,
    pub child_turn_index: usize,
    pub primer_turn_index: usize,
}

#[derive(Serialize, Clone)]
pub struct VoiceExitEvent {
    pub reason: String,
}

#[derive(Serialize, Clone)]
pub struct VoiceInferenceErrorEvent {
    pub message: String,
}

pub struct TauriEventObserver<R: tauri::Runtime = tauri::Wry> {
    app: tauri::AppHandle<R>,
}

impl<R: tauri::Runtime> TauriEventObserver<R> {
    pub fn new(app: tauri::AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: tauri::Runtime + 'static> LoopObserver for TauriEventObserver<R> {
    fn on_state_change(&mut self, state: VoiceState, hint: Option<&str>) {
        let payload = StateChangeEvent {
            state: state.name().to_string(),
            hint: hint.map(String::from),
        };
        if let Err(e) = self.app.emit("primer://voice/state_change", &payload) {
            tracing::warn!("emit primer://voice/state_change failed: {e}");
        }
    }

    fn on_transcript_finalized(&mut self, text: &str) {
        let payload = TranscriptEvent {
            text: text.to_string(),
        };
        if let Err(e) = self.app.emit("primer://voice/transcript", &payload) {
            tracing::warn!("emit primer://voice/transcript failed: {e}");
        }
    }

    fn on_response_chunk(&mut self, primer_turn_index: usize, chunk: &str) {
        let payload = ResponseChunkEvent {
            primer_turn_index,
            text: chunk.to_string(),
        };
        if let Err(e) = self.app.emit("primer://voice/response_chunk", &payload) {
            tracing::warn!("emit primer://voice/response_chunk failed: {e}");
        }
    }

    fn on_response_complete(&mut self, payload: TurnCompletePayload) {
        let evt = ResponseCompleteEvent {
            session_id: payload.session_id,
            child_turn_index: payload.child_turn_index,
            primer_turn_index: payload.primer_turn_index,
        };
        if let Err(e) = self.app.emit("primer://voice/response_complete", &evt) {
            tracing::warn!("emit primer://voice/response_complete failed: {e}");
        }
    }

    fn on_inference_error(&mut self, err: &primer_core::error::InferenceError) {
        let evt = VoiceInferenceErrorEvent {
            message: format!("{err:?}"),
        };
        if let Err(e) = self.app.emit("primer://voice/inference_error", &evt) {
            tracing::warn!("emit primer://voice/inference_error failed: {e}");
        }
    }

    fn on_exit(&mut self, reason: ExitReason) {
        let evt = VoiceExitEvent {
            reason: reason.name().to_string(),
        };
        if let Err(e) = self.app.emit("primer://voice/exit", &evt) {
            tracing::warn!("emit primer://voice/exit failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `tauri::test::mock_app` constructs an AppHandle that records emitted
    /// events into a buffer we can read back. Pin the wire shape: a JSON
    /// state_change payload must carry exactly `state` (and optionally
    /// `hint`), nothing more.
    #[test]
    fn state_change_event_serialises_to_expected_json() {
        let evt = StateChangeEvent {
            state: "listen".to_string(),
            hint: None,
        };
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json, serde_json::json!({"state": "listen"}));
    }

    #[test]
    fn state_change_with_hint_includes_hint_field() {
        let evt = StateChangeEvent {
            state: "listen".to_string(),
            hint: Some("user_cancel".to_string()),
        };
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"state": "listen", "hint": "user_cancel"})
        );
    }

    #[test]
    fn response_chunk_event_shape() {
        let evt = ResponseChunkEvent {
            primer_turn_index: 3,
            text: "hello".to_string(),
        };
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"primer_turn_index": 3, "text": "hello"})
        );
    }
}
