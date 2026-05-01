//! Default values for `ClassifierSettings`. Per the no-magic-numbers
//! convention, every numeric used by the classifier subsystem is
//! defined here (or in a sibling settings struct field).

pub const DEFAULT_HISTORY_DEPTH: usize = 3;
pub const DEFAULT_BLOCKING_TIMEOUT_MS: u64 = 500;
pub const DEFAULT_CONFIDENCE_THRESHOLD: f32 = 0.6;
pub const DEFAULT_RECENT_CHILD_TURNS_FOR_CLASSIFICATION: usize = 3;
pub const DEFAULT_MAX_CLASSIFIER_OUTPUT_CHARS: usize = 512;
pub const DEFAULT_CLASSIFIER_MAX_TOKENS: u32 = 256;
pub const DEFAULT_CLASSIFIER_TEMPERATURE: f32 = 0.2;
pub const DEFAULT_CLASSIFIER_TOP_P: f32 = 0.9;
