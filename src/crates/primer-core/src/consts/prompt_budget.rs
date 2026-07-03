//! Token-budget defaults for small-context prompt assembly (the Qualcomm
//! NPU `QnnBackend` runs a 2048-token Genie context). Consumed by
//! [`crate::prompt_budget`] and the dialogue manager's `build_turn_prompt`
//! when [`crate::backend::is_small_context_backend`] is true.

/// Characters-per-token proxy for the tokenizer-free estimate in
/// [`crate::prompt_budget::estimate_tokens`]. English/German average
/// ~3.5–4 chars/token; 4 keeps the estimate conservative (slightly
/// under-counts), which the on-device `genie.log` prompt-token line
/// calibrates against. Phase 1.2 step "fit 2K context".
pub const CHARS_PER_TOKEN: usize = 4;

/// Maximum tokens per knowledge-base passage injected into a
/// small-context system prompt. Whole wiki/seed passages run 50–520
/// tokens each; truncating to ~110 tokens (≈440 chars — roughly a
/// lead paragraph) keeps the grounding while three passages cost
/// ~330 tokens instead of ~900. The truncation is sentence-boundary
/// aware so a passage still reads as coherent context.
pub const KB_PASSAGE_MAX_TOKENS_SMALL_CONTEXT: usize = 110;

/// Token ceiling for the *system prompt* on a small-context backend.
/// The 2048-token Genie context must hold the system prompt, the
/// recent-turn chat messages (≤ `DEFAULT_CONTEXT_WINDOW_TURNS_SMALL_CONTEXT`
/// exchanges), and leave room for the reply. Budgeting the system
/// prompt to ~1100 tokens reserves ~950 for messages + reply
/// (Socratic replies are short — they ask more than they answer).
/// The dialogue manager keeps the pedagogical core (base prompt +
/// intent + engagement) unconditionally and drops/truncates the
/// optional sections (KB, summary, retrieved turns, vocab) to fit.
/// Calibrated against the on-device `genie.log` "Context limit
/// exceeded (P + G > C)" line.
pub const SYSTEM_PROMPT_BUDGET_TOKENS_SMALL_CONTEXT: usize = 1100;
