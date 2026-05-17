//! Speech-recognition authorization probe.
//!
//! Wraps Apple's `+[SFSpeechRecognizer requestAuthorization:]` which calls
//! back asynchronously on the main thread with an `SFSpeechRecognizerAuthorizationStatus`.
//! We bridge that into a Rust `tokio::sync::oneshot` so callers can await
//! the result naturally.

use objc2_speech::{SFSpeechRecognizer, SFSpeechRecognizerAuthorizationStatus};
use tokio::sync::oneshot;

/// Authorization decision returned by `request_speech_authorization`.
///
/// Mirrors `SFSpeechRecognizerAuthorizationStatus` one-for-one so callers
/// don't have to import the objc2 type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechAuthStatus {
    /// User has not yet been asked, or hasn't decided.
    NotDetermined,
    /// Restricted by parental controls / MDM. Treat as a hard refusal.
    Restricted,
    /// User explicitly denied. Treat as a hard refusal.
    Denied,
    /// Authorized to use speech recognition.
    Authorized,
}

impl From<SFSpeechRecognizerAuthorizationStatus> for SpeechAuthStatus {
    fn from(raw: SFSpeechRecognizerAuthorizationStatus) -> Self {
        // Apple's documented mapping (stable across iOS/macOS):
        //   0 = notDetermined, 1 = denied, 2 = restricted, 3 = authorized
        match raw {
            SFSpeechRecognizerAuthorizationStatus::NotDetermined => SpeechAuthStatus::NotDetermined,
            SFSpeechRecognizerAuthorizationStatus::Denied => SpeechAuthStatus::Denied,
            SFSpeechRecognizerAuthorizationStatus::Restricted => SpeechAuthStatus::Restricted,
            SFSpeechRecognizerAuthorizationStatus::Authorized => SpeechAuthStatus::Authorized,
            _ => SpeechAuthStatus::Denied,
        }
    }
}

/// Return the app's current speech-recognition authorization status without
/// prompting the user.
///
/// Safe to call without `NSSpeechRecognitionUsageDescription` in Info.plist.
pub fn current_speech_authorization_status() -> SpeechAuthStatus {
    // SAFETY: pure class-method read, no side effects.
    SpeechAuthStatus::from(unsafe { SFSpeechRecognizer::authorizationStatus() })
}

/// Request authorization to use SFSpeechRecognizer. Triggers the OS consent
/// prompt on first call; subsequent calls return the cached decision.
///
/// The OS callback fires on the main thread; we forward it through a
/// oneshot channel so the awaiter can be on any tokio worker.
///
/// # Panics
///
/// The process will abort if `NSSpeechRecognitionUsageDescription` is not
/// present in the app's `Info.plist`. This is an OS-level enforcement — it
/// does not apply to the current status query (see
/// `current_speech_authorization_status`).
pub async fn request_speech_authorization() -> SpeechAuthStatus {
    let (tx, rx) = oneshot::channel::<SpeechAuthStatus>();
    let tx_cell = std::sync::Mutex::new(Some(tx));

    let cb = block2::RcBlock::new(move |status: SFSpeechRecognizerAuthorizationStatus| {
        if let Some(tx) = tx_cell.lock().unwrap().take() {
            let _ = tx.send(SpeechAuthStatus::from(status));
        }
    });

    // SAFETY: requestAuthorization: takes a block that the OS retains;
    // RcBlock derefs to Block (= DynBlock), which is what the API expects.
    // RcBlock's drop semantics keep it alive until the OS releases it.
    unsafe { SFSpeechRecognizer::requestAuthorization(&cb) };

    rx.await.unwrap_or(SpeechAuthStatus::Denied)
}
