//! Per-locale voice profile defaults for the shared voice loop.
//!
//! Stub — full implementation lands in a later task.

/// A locale-specific default voice profile.
#[derive(Debug, Clone)]
pub struct LocaleDefault {
    pub locale: &'static str,
    pub model_id: &'static str,
}

/// Slice of known locale defaults. Empty until a later task populates it.
pub const LOCALE_DEFAULTS: &[LocaleDefault] = &[];

/// Returns the default voice for `locale`, or `None` if no default is registered.
pub fn voice_default_for(_locale: &str) -> Option<&'static LocaleDefault> {
    LOCALE_DEFAULTS.iter().find(|d| d.locale == _locale)
}
