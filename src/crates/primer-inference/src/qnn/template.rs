//! Jinja2 chat-template renderer.
//!
//! Wraps [`minijinja`] to turn a [`primer_core::inference::Prompt`] into
//! the single rendered string the Genie API takes. Phase 1.2 design §4:
//!
//! > Genie's `GenieDialog_query` accepts a single rendered prompt string —
//! > there is no message-list API. The Primer's `Prompt { system, messages }`
//! > shape must be flattened to the chat template the exported model
//! > expects.
//!
//! The template is loaded from `primer-meta.json::chat_template` at
//! backend startup; rendering happens per `generate_stream` call.

use std::sync::Arc;

use minijinja::{Environment, context};
use primer_core::inference::{Prompt, Role};
use serde::Serialize;
use thiserror::Error;

/// Internal template name registered with the [`Environment`]. Single
/// template per [`ChatTemplate`] instance — minijinja doesn't allow
/// rendering an unregistered template by string.
const CHAT_TEMPLATE_NAME: &str = "chat";

/// Compiled Jinja2 chat template, ready to render against a [`Prompt`].
///
/// Construction parses the template string once; [`Self::render`] is
/// cheap. The compiled [`Environment`] is held via `Arc` so the
/// `ChatTemplate` is cheap to clone, which keeps the [`super::backend::QnnBackend`]
/// struct cheap to share through `Arc`.
#[derive(Clone)]
pub struct ChatTemplate {
    env: Arc<Environment<'static>>,
}

impl std::fmt::Debug for ChatTemplate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `Environment` doesn't impl `Debug`; render a minimal stand-in so
        // call sites that derive `Debug` for the parent struct still
        // compile.
        f.debug_struct("ChatTemplate").finish_non_exhaustive()
    }
}

impl ChatTemplate {
    /// Parse the given Jinja2 template string.
    ///
    /// Returns [`TemplateError::Compile`] when the syntax is malformed —
    /// surface this to the user as an "invalid chat_template in
    /// primer-meta.json" hint.
    pub fn compile(template_str: &str) -> Result<Self, TemplateError> {
        let mut env = Environment::new();
        env.add_template_owned(CHAT_TEMPLATE_NAME, template_str.to_string())
            .map_err(|source| TemplateError::Compile { source })?;
        Ok(Self { env: Arc::new(env) })
    }

    /// Render the prompt into a single string, suitable for handing to
    /// Genie. `add_generation_prompt = true` is exposed to the template
    /// so the standard "open the next assistant turn" idiom works
    /// (`{% if add_generation_prompt %}<|im_start|>assistant\n{% endif %}`).
    pub fn render(&self, prompt: &Prompt) -> Result<String, TemplateError> {
        let messages = flatten_messages(prompt);
        let tmpl = self
            .env
            .get_template(CHAT_TEMPLATE_NAME)
            .map_err(|source| TemplateError::Render { source })?;
        tmpl.render(context! {
            messages => messages,
            add_generation_prompt => true,
        })
        .map_err(|source| TemplateError::Render { source })
    }
}

/// Pure helper: turn a [`Prompt`] into the flat `[{role, content}]`
/// list every chat template iterates over. The non-empty system field
/// becomes the leading `system`-role message.
///
/// Exposed so tests can pin the role mapping without going through a
/// template render.
pub fn flatten_messages(prompt: &Prompt) -> Vec<MessageView<'_>> {
    let mut out: Vec<MessageView<'_>> = Vec::with_capacity(prompt.messages.len() + 1);
    if !prompt.system.is_empty() {
        out.push(MessageView {
            role: "system",
            content: &prompt.system,
        });
    }
    for m in &prompt.messages {
        out.push(MessageView {
            role: role_str(m.role),
            content: &m.content,
        });
    }
    out
}

/// Pure mapping from the closed [`Role`] enum to the canonical lowercase
/// role string every chat template expects. Exposed so the role-string
/// convention is one source of truth.
pub fn role_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

/// Borrowed message view rendered into Jinja2 context. Mirrors the
/// shape every published chat template iterates over (`{{ m.role }}` /
/// `{{ m.content }}`). Kept borrowed to avoid cloning content strings on
/// the hot per-turn render path.
#[derive(Debug, Serialize)]
pub struct MessageView<'a> {
    pub role: &'static str,
    pub content: &'a str,
}

/// Errors [`ChatTemplate`] can return.
#[derive(Debug, Error)]
pub enum TemplateError {
    /// The template string failed to parse. Almost always a bug in
    /// `primer-meta.json::chat_template` — surface to the user as a
    /// load-time error rather than waiting for the first inference call.
    #[error("chat template compile failed: {source}")]
    Compile {
        #[source]
        source: minijinja::Error,
    },

    /// The template parsed but rendering this specific [`Prompt`] failed.
    /// Possible causes: a template field reference (`{{ foo.bar }}`)
    /// against a context that doesn't carry `foo`. Rare in practice; the
    /// Primer always supplies `messages` and `add_generation_prompt`.
    #[error("chat template render failed: {source}")]
    Render {
        #[source]
        source: minijinja::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use primer_core::inference::{Message, Role};

    /// ChatML template — Qwen3, Yi, Mistral-Instruct, others.
    const CHATML_TEMPLATE: &str = "{% for m in messages %}<|im_start|>{{ m.role }}\n{{ m.content }}<|im_end|>\n{% endfor %}{% if add_generation_prompt %}<|im_start|>assistant\n{% endif %}";

    /// Llama-3-Instruct template — Meta's Llama-3.x family.
    /// The double newline after each role header is part of the spec;
    /// don't normalise it away.
    const LLAMA3_INSTRUCT_TEMPLATE: &str = "<|begin_of_text|>{% for m in messages %}<|start_header_id|>{{ m.role }}<|end_header_id|>\n\n{{ m.content }}<|eot_id|>{% endfor %}{% if add_generation_prompt %}<|start_header_id|>assistant<|end_header_id|>\n\n{% endif %}";

    fn helpful_prompt() -> Prompt {
        Prompt {
            system: "You are a helpful Socratic tutor.".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: "Why is the sky blue?".to_string(),
            }],
        }
    }

    #[test]
    fn role_str_maps_closed_enum_to_lowercase() {
        assert_eq!(role_str(Role::System), "system");
        assert_eq!(role_str(Role::User), "user");
        assert_eq!(role_str(Role::Assistant), "assistant");
    }

    #[test]
    fn flatten_prepends_system_message_when_non_empty() {
        let prompt = helpful_prompt();
        let flat = flatten_messages(&prompt);
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[0].role, "system");
        assert_eq!(flat[0].content, "You are a helpful Socratic tutor.");
        assert_eq!(flat[1].role, "user");
        assert_eq!(flat[1].content, "Why is the sky blue?");
    }

    #[test]
    fn flatten_skips_system_when_empty() {
        let prompt = Prompt {
            system: String::new(),
            messages: vec![Message {
                role: Role::User,
                content: "hi".to_string(),
            }],
        };
        let flat = flatten_messages(&prompt);
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].role, "user");
    }

    #[test]
    fn renders_chatml_with_system_and_user() {
        let tmpl = ChatTemplate::compile(CHATML_TEMPLATE).unwrap();
        let rendered = tmpl.render(&helpful_prompt()).unwrap();
        // System block.
        assert!(
            rendered.contains("<|im_start|>system\nYou are a helpful Socratic tutor.<|im_end|>"),
            "missing system block: {rendered}",
        );
        // User block.
        assert!(
            rendered.contains("<|im_start|>user\nWhy is the sky blue?<|im_end|>"),
            "missing user block: {rendered}",
        );
        // Trailing assistant-open tag invites generation.
        assert!(
            rendered.trim_end().ends_with("<|im_start|>assistant"),
            "missing assistant-open tail: {rendered}",
        );
    }

    #[test]
    fn renders_llama3_instruct_with_system_and_user() {
        let tmpl = ChatTemplate::compile(LLAMA3_INSTRUCT_TEMPLATE).unwrap();
        let rendered = tmpl.render(&helpful_prompt()).unwrap();
        // Begin-of-text prefix is mandatory in Llama-3.
        assert!(rendered.starts_with("<|begin_of_text|>"), "{rendered}");
        // System header + body + EOT.
        assert!(
            rendered.contains("<|start_header_id|>system<|end_header_id|>\n\nYou are a helpful Socratic tutor.<|eot_id|>"),
            "missing system header block: {rendered}",
        );
        // User header + body + EOT.
        assert!(
            rendered.contains(
                "<|start_header_id|>user<|end_header_id|>\n\nWhy is the sky blue?<|eot_id|>"
            ),
            "missing user header block: {rendered}",
        );
        // Trailing assistant header invites generation.
        assert!(
            rendered
                .trim_end()
                .ends_with("<|start_header_id|>assistant<|end_header_id|>"),
            "missing assistant-open tail: {rendered}",
        );
    }

    #[test]
    fn renders_chatml_when_only_user_message_is_present() {
        let tmpl = ChatTemplate::compile(CHATML_TEMPLATE).unwrap();
        let prompt = Prompt {
            system: String::new(),
            messages: vec![Message {
                role: Role::User,
                content: "hello".to_string(),
            }],
        };
        let rendered = tmpl.render(&prompt).unwrap();
        assert!(
            !rendered.contains("system"),
            "should skip empty system block: {rendered}"
        );
        assert!(rendered.contains("<|im_start|>user\nhello<|im_end|>"));
    }

    #[test]
    fn renders_chatml_with_multi_turn_history() {
        let tmpl = ChatTemplate::compile(CHATML_TEMPLATE).unwrap();
        let prompt = Prompt {
            system: "You are a tutor.".to_string(),
            messages: vec![
                Message {
                    role: Role::User,
                    content: "What is gravity?".to_string(),
                },
                Message {
                    role: Role::Assistant,
                    content: "What do you think makes things fall?".to_string(),
                },
                Message {
                    role: Role::User,
                    content: "I dunno, magic?".to_string(),
                },
            ],
        };
        let rendered = tmpl.render(&prompt).unwrap();
        // Each turn must appear once, in order.
        let positions: Vec<usize> = [
            "You are a tutor.",
            "What is gravity?",
            "What do you think makes things fall?",
            "I dunno, magic?",
        ]
        .into_iter()
        .map(|needle| {
            rendered
                .find(needle)
                .unwrap_or_else(|| panic!("missing {needle:?} in {rendered}"))
        })
        .collect();
        for w in positions.windows(2) {
            assert!(w[0] < w[1], "turn order broken in {rendered}");
        }
    }

    #[test]
    fn render_preserves_unicode_and_special_chars() {
        // Children and the Primer can produce content with quotes, emojis,
        // markdown markers, math operators. The template must pass these
        // through verbatim — Jinja2 doesn't HTML-escape by default but
        // pinning the behaviour catches an accidental escape config.
        let tmpl = ChatTemplate::compile(CHATML_TEMPLATE).unwrap();
        let prompt = Prompt {
            system: String::new(),
            messages: vec![Message {
                role: Role::User,
                content: "He said \"5 < 7\" & it's 🌟 *important*".to_string(),
            }],
        };
        let rendered = tmpl.render(&prompt).unwrap();
        assert!(
            rendered.contains("He said \"5 < 7\" & it's 🌟 *important*"),
            "special chars mangled: {rendered}",
        );
    }

    #[test]
    fn compile_returns_compile_error_for_malformed_template() {
        // Unclosed `{% for %}` block should fail at compile, not at
        // first render — we want startup errors, not in-conversation
        // surprises.
        let err = ChatTemplate::compile("{% for m in messages %}{{ m.role }}").unwrap_err();
        assert!(matches!(err, TemplateError::Compile { .. }), "got {err:?}");
    }

    #[test]
    fn fallback_template_from_meta_renders_chatml_shape() {
        // The `FALLBACK_CHAT_TEMPLATE` const exposed by `qnn::meta` is what
        // load-or-fallback substitutes when `primer-meta.json` is absent.
        // Pin that it actually compiles and renders sensibly — a syntax
        // typo in the fallback would silently brick every meta-less bundle.
        use super::super::meta::FALLBACK_CHAT_TEMPLATE;
        let tmpl = ChatTemplate::compile(FALLBACK_CHAT_TEMPLATE).unwrap();
        let rendered = tmpl.render(&helpful_prompt()).unwrap();
        assert!(rendered.contains("<|im_start|>system"));
        assert!(rendered.contains("<|im_start|>user"));
        assert!(rendered.trim_end().ends_with("<|im_start|>assistant"));
    }
}
