//! Token-budget helpers for assembling prompts that must fit a small
//! context window (the Qualcomm NPU `QnnBackend` runs a 2048-token
//! Genie context — see [`crate::backend::is_small_context_backend`]).
//!
//! The pedagogy layer has no real tokenizer, so these helpers use a
//! `chars / CHARS_PER_TOKEN` proxy. It is deliberately conservative for
//! English/German (which average ~3.5–4 chars per token) and good
//! enough to *gate* assembly — the QNN backend owns the exact tokenizer
//! if precise counting is ever needed. The proxy is calibrated against
//! the on-device `genie.log` "Context limit exceeded (P + G > C)" line.
//!
//! All functions here are pure: no I/O, no clock, no allocation beyond
//! the returned `String`. They are the single source of truth for the
//! small-context trimming the dialogue manager applies — keep the
//! decision logic here and unit-tested, not inline in `build_turn_prompt`.

use crate::consts::prompt_budget::CHARS_PER_TOKEN;

/// Estimate the token count of `text` using the `chars / CHARS_PER_TOKEN`
/// proxy, rounded up so a non-empty string never estimates to zero tokens.
///
/// Counts Unicode scalar values (`chars`), not bytes, so multi-byte
/// German/Hindi text isn't over-counted.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(CHARS_PER_TOKEN)
}

/// Truncate `text` so its [`estimate_tokens`] is at most `max_tokens`,
/// cutting at the cleanest boundary that fits:
///
/// 1. the end of the last sentence (`.`/`!`/`?`) that fits, else
/// 2. the last whitespace boundary that fits, else
/// 3. a hard character cut at the budget.
///
/// Never cuts mid-word when a word boundary is available. Returns the
/// input unchanged when it already fits. The result is trimmed of
/// trailing whitespace. This is how whole wiki/seed passages are shrunk
/// to their most relevant lead for a small-context prompt.
pub fn truncate_to_tokens(text: &str, max_tokens: usize) -> String {
    if estimate_tokens(text) <= max_tokens {
        return text.to_string();
    }
    let max_chars = max_tokens.saturating_mul(CHARS_PER_TOKEN);
    // Walk to the char-budget boundary by char index (not byte index)
    // so multi-byte scalars are handled correctly.
    let mut boundary_byte = text.len();
    for (count, (byte_idx, _)) in text.char_indices().enumerate() {
        if count == max_chars {
            boundary_byte = byte_idx;
            break;
        }
    }
    let window = &text[..boundary_byte];

    // Prefer the last sentence terminator within the window.
    if let Some(end) = window.rfind(['.', '!', '?']) {
        // Include the terminator itself.
        return window[..=end].trim_end().to_string();
    }
    // Fall back to the last whitespace boundary.
    if let Some(space) = window.rfind(char::is_whitespace) {
        let trimmed = window[..space].trim_end();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    // No boundary available — hard cut at the budget.
    window.trim_end().to_string()
}

/// Greedily decide which optional prompt sections fit within
/// `remaining_budget` tokens, given each section's estimated cost in
/// **value order** (most valuable first).
///
/// Returns a `Vec<bool>` parallel to `section_tokens`: `true` = include.
/// A section is included when adding it keeps the running total within
/// budget; otherwise it is skipped and the next (cheaper) section still
/// gets a chance. This keeps a cheap high-value-enough tail section even
/// when an earlier expensive one was dropped.
///
/// The caller is responsible for ordering sections by pedagogical value
/// (e.g. knowledge passages before vocab-review hints) — this function
/// only does the arithmetic.
pub fn select_sections(remaining_budget: usize, section_tokens: &[usize]) -> Vec<bool> {
    let mut used = 0usize;
    section_tokens
        .iter()
        .map(|&cost| {
            if used + cost <= remaining_budget {
                used += cost;
                true
            } else {
                false
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_tokens_rounds_up_nonempty() {
        assert_eq!(estimate_tokens(""), 0);
        // 1..=4 chars → 1 token (ceil of 4/4=1, 1/4=0.25→1).
        assert_eq!(estimate_tokens("a"), 1);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }

    #[test]
    fn estimate_tokens_counts_chars_not_bytes() {
        // "über" is 4 chars but 5 bytes (ü = 2 bytes). Must be 1 token.
        assert_eq!("über".len(), 5);
        assert_eq!(estimate_tokens("über"), 1);
    }

    #[test]
    fn truncate_returns_input_when_within_budget() {
        let s = "Short enough.";
        assert_eq!(truncate_to_tokens(s, 100), s);
    }

    #[test]
    fn truncate_prefers_sentence_boundary() {
        // Budget 6 tok ≈ 24 chars → window "Atoms are tiny. They mak".
        // The sentence terminator (after "tiny") sits EARLIER than the
        // last whitespace boundary ("...They"), so this input genuinely
        // discriminates the sentence-boundary branch from the whitespace
        // fallback: word-boundary cutting would yield "Atoms are tiny. They".
        let s = "Atoms are tiny. They make up everything around us in the world.";
        let out = truncate_to_tokens(s, 6);
        assert_eq!(out, "Atoms are tiny.");
    }

    #[test]
    fn truncate_falls_back_to_word_boundary() {
        // No sentence terminator within the window → cut at whitespace.
        let s = "alpha beta gamma delta epsilon zeta eta theta";
        let out = truncate_to_tokens(s, 3); // 3 tok ≈ 12 chars
        // Within 12 chars: "alpha beta g" → last space before that.
        assert_eq!(out, "alpha beta");
        // Never ends mid-word.
        assert!(!out.ends_with('g'));
    }

    #[test]
    fn truncate_hard_cuts_when_no_boundary() {
        let s = "supercalifragilisticexpialidocious";
        let out = truncate_to_tokens(s, 2); // ≈ 8 chars, no space/terminator
        assert_eq!(out.chars().count(), 8);
        assert_eq!(out, "supercal");
    }

    #[test]
    fn truncate_result_fits_budget() {
        let s = "Sentence one is here. Sentence two is also here. And a third.";
        for budget in 1..=20 {
            let out = truncate_to_tokens(s, budget);
            assert!(
                estimate_tokens(&out) <= budget,
                "budget={budget}, out={out:?} estimates {}",
                estimate_tokens(&out)
            );
        }
    }

    #[test]
    fn select_sections_includes_while_fitting() {
        // Budget 10; costs 4,4,4 → keep first two (8), drop third (12>10).
        assert_eq!(select_sections(10, &[4, 4, 4]), vec![true, true, false]);
    }

    #[test]
    fn select_sections_skips_too_big_keeps_cheaper_tail() {
        // Budget 5; first section costs 9 (skip), second costs 3 (keep).
        assert_eq!(select_sections(5, &[9, 3]), vec![false, true]);
    }

    #[test]
    fn select_sections_zero_budget_includes_nothing() {
        assert_eq!(select_sections(0, &[1, 0, 2]), vec![false, true, false]);
    }

    #[test]
    fn select_sections_empty_is_empty() {
        assert_eq!(select_sections(100, &[]), Vec::<bool>::new());
    }
}
