//! Defaults for reasoning-token stripping (see [`crate::reasoning`]).

/// `(open, close)` marker pairs stripped by default on every Ollama /
/// openai-compat stream. A non-reasoning model never emits these, so the
/// filter is a no-op when they are absent.
///
/// - `<think>…</think>`: DeepSeek-R1, QwQ, Qwen3, de-facto convention.
/// - `<|channel>…<channel|>`: Gemma4 thinking channel. Per the ollama
///   gemma4 docs the output is `<|channel>thought\n[reasoning]<channel|>`
///   with the final answer OUTSIDE the markers (disabled-mode example:
///   `<|channel>thought\n<channel|>[Final answer]`). Note the asymmetry:
///   open is `<|channel>` (pipe after `<`), close is `<channel|>` (pipe
///   before `>`). Stripping the channel removes the `thought\n` label too;
///   the visible answer survives because it is outside the pair.
pub const DEFAULT_MARKERS: &[(&str, &str)] =
    &[("<think>", "</think>"), ("<|channel>", "<channel|>")];
