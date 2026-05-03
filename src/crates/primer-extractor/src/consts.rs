//! Default values for `ExtractorSettings`. Per the no-magic-numbers
//! convention, every numeric used by the extractor subsystem is
//! defined here (or in a sibling settings struct field).

pub const DEFAULT_BLOCKING_TIMEOUT_MS: u64 = 1500;
pub const DEFAULT_RECENT_CONTEXT_TURNS: usize = 4;
pub const DEFAULT_MAX_EXTRACTOR_OUTPUT_CHARS: usize = 1024;
pub const DEFAULT_MAX_CONCEPTS_PER_SPEAKER: usize = 8;
pub const DEFAULT_EXTRACTOR_MAX_TOKENS: u32 = 384;
pub const DEFAULT_EXTRACTOR_TEMPERATURE: f32 = 0.1;
pub const DEFAULT_EXTRACTOR_TOP_P: f32 = 0.9;
