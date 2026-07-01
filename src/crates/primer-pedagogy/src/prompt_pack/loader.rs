//! Prompt-pack loading entry points and the preview-status warning gate.
//!
//! Packs are embedded at compile time via `include_str!` and parsed on
//! demand. `PRIMER_PROMPTS_DIR` overrides the embedded body with an
//! on-disk file for translator iteration. [`load`] parses freshly every
//! call; [`load_cached`] returns a process-wide cached instance for the
//! production hot path and emits a one-time warning per preview locale.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

use primer_core::error::{PrimerError, Result};
use primer_core::i18n::Locale;

use super::toml_pack::TomlPromptPack;
use super::{PackStatus, PromptPack};

/// Per-locale packs embedded at compile time so a binary can ship
/// without any data files alongside it. Override at runtime via
/// `PRIMER_PROMPTS_DIR`.
const EN_TOML: &str = include_str!("../../prompts/en.toml");
const DE_TOML: &str = include_str!("../../prompts/de.toml");
const HI_TOML: &str = include_str!("../../prompts/hi.toml");

fn embedded_pack(locale: Locale) -> &'static str {
    match locale {
        Locale::English => EN_TOML,
        Locale::German => DE_TOML,
        Locale::Hindi => HI_TOML,
    }
}

/// Per-process gate: tracks which preview locales have already emitted
/// their one-time warning. Populated by `emit_preview_warning_if_first`;
/// consulted by `load_cached`.
fn preview_warned_gate() -> &'static Mutex<HashSet<Locale>> {
    static GATE: OnceLock<Mutex<HashSet<Locale>>> = OnceLock::new();
    GATE.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Emit the preview-status warning for `locale` if it hasn't been emitted
/// before in this process. Idempotent across calls; the warning text
/// names the locale's pack_id so `tail -f` of logs can tell apart
/// concurrent preview locales.
pub(crate) fn emit_preview_warning_if_first(locale: Locale) {
    // Decide whether to emit inside a tight scope so the mutex guard is
    // released before `tracing::warn!` runs (a synchronously-writing
    // subscriber would otherwise hold the gate for the warn's duration).
    // Poison fallback: treat as "first time" and warn anyway — the spec
    // requires degrading gracefully, never silencing.
    let is_first = match preview_warned_gate().lock() {
        Ok(mut seen) => seen.insert(locale),
        Err(_) => true,
    };
    if is_first {
        tracing::warn!(
            target: "primer::prompt_pack",
            locale = locale.pack_id(),
            "prompt pack is in preview status — machine-translated content awaiting native-speaker review. \
             This locale is not in Locale::ALL and is not advertised to end users."
        );
    }
}

#[cfg(test)]
pub(crate) fn reset_preview_warn_once_for_test(locale: Locale) {
    let mut seen = preview_warned_gate()
        .lock()
        .expect("preview gate mutex poisoned");
    seen.remove(&locale);
}

/// Load the prompt pack for `locale`, freshly parsing every call.
///
/// Lookup order:
/// 1. If `PRIMER_PROMPTS_DIR` is set, read `<dir>/<pack_id>.toml`.
/// 2. Otherwise, parse the compile-time-embedded pack.
///
/// Returns `Err` on I/O failure, TOML-parse failure, placeholder
/// validation failure, missing-intent variants, or meta-inconsistency
/// against `Locale`'s projections. All pack-shape errors are surfaced as
/// `PrimerError::Config` so a broken pack fails loudly at startup.
///
/// Use [`load_cached`] for the production hot path; reserve `load` for
/// tests and PRIMER_PROMPTS_DIR-driven translator iteration.
pub fn load(locale: Locale) -> Result<Arc<dyn PromptPack>> {
    let raw = match std::env::var("PRIMER_PROMPTS_DIR") {
        Ok(dir) => {
            let path = std::path::Path::new(&dir).join(format!("{}.toml", locale.pack_id()));
            std::fs::read_to_string(&path).map_err(|e| {
                PrimerError::Config(format!(
                    "PRIMER_PROMPTS_DIR set but {} could not be read: {e}",
                    path.display()
                ))
            })?
        }
        Err(_) => embedded_pack(locale).to_string(),
    };
    let pack = TomlPromptPack::from_toml_str(locale, &raw)?;
    Ok(Arc::new(pack))
}

/// Load the prompt pack for `locale`, returning a process-wide cached
/// instance after the first successful load.
///
/// When `PRIMER_PROMPTS_DIR` is set the cache is bypassed so translator
/// iteration sees fresh content on every call. Otherwise every caller
/// shares the same `Arc<dyn PromptPack>`, sidestepping a per-session
/// re-parse of the embedded TOML for callers like `DialogueManager::new`
/// that construct the pack but never need to mutate it.
pub fn load_cached(locale: Locale) -> Result<Arc<dyn PromptPack>> {
    // PRIMER_PROMPTS_DIR is the translator-iteration escape hatch; honour
    // it by bypassing the cache so a re-saved TOML file is reflected on
    // the next `load_cached` call.
    if std::env::var_os("PRIMER_PROMPTS_DIR").is_some() {
        return load(locale);
    }
    static EN_PACK: OnceLock<Arc<dyn PromptPack>> = OnceLock::new();
    static DE_PACK: OnceLock<Arc<dyn PromptPack>> = OnceLock::new();
    static HI_PACK: OnceLock<Arc<dyn PromptPack>> = OnceLock::new();
    let pack = match locale {
        Locale::English => {
            if let Some(p) = EN_PACK.get() {
                Arc::clone(p)
            } else {
                let p = load(locale)?;
                let _ = EN_PACK.set(Arc::clone(&p));
                p
            }
        }
        Locale::German => {
            if let Some(p) = DE_PACK.get() {
                Arc::clone(p)
            } else {
                let p = load(locale)?;
                let _ = DE_PACK.set(Arc::clone(&p));
                p
            }
        }
        Locale::Hindi => {
            if let Some(p) = HI_PACK.get() {
                Arc::clone(p)
            } else {
                let p = load(locale)?;
                let _ = HI_PACK.set(Arc::clone(&p));
                p
            }
        }
    };
    if pack.status() == PackStatus::Preview {
        emit_preview_warning_if_first(locale);
    }
    Ok(pack)
}
