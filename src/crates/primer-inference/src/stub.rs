//! Stub inference backend — returns canned responses for testing.
//!
//! Use this to develop and test the pedagogical engine without
//! needing a model loaded. The stub echoes the last user message
//! wrapped in a Socratic question, which is enough to exercise
//! the dialogue state machine.

use async_trait::async_trait;
use futures::stream;
use primer_core::error::Result;
use primer_core::inference::*;

pub struct StubBackend;

#[async_trait]
impl InferenceBackend for StubBackend {
    fn name(&self) -> &str {
        "stub"
    }

    async fn is_available(&self) -> bool {
        true
    }

    async fn generate_stream(
        &self,
        prompt: &Prompt,
        _params: &GenerationParams,
    ) -> Result<TokenStream> {
        // Extract the last user message to create a plausible Socratic response.
        let last_user_msg = prompt
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(|m| m.content.as_str())
            .unwrap_or("something");

        let response = format!(
            "That's an interesting thought about {last_user_msg}. \
             But can you tell me *why* you think that's the case? \
             What would happen if it were different?"
        );

        let chunk = TokenChunk {
            text: response,
            done: true,
        };

        Ok(Box::pin(stream::once(async { Ok(chunk) })))
    }
}
