//! Loading the benchmark prompt corpus from JSONL.
//!
//! Each line of `data/bench/socratic_prompts.jsonl` is a JSON object
//! describing one dialogue-continuation prompt: a `label`, an optional
//! `system` override, and a non-empty list of `turns` (reusing
//! [`primer_core::inference::Message`], so `role` is one of
//! `system`/`user`/`assistant`). A line that omits `system` inherits
//! [`DEFAULT_BENCH_SYSTEM_PROMPT`]. The parser is pure and line-numbered so
//! a malformed corpus points at the offending line; the file read is the
//! only I/O and lives in [`load_bench_prompts`].

use std::path::{Path, PathBuf};

use primer_core::inference::{Message, Prompt, Role};
use serde::Deserialize;
use thiserror::Error;

/// System prompt applied to a bench line that omits its own `system`.
///
/// Deliberately compact (a few sentences, not the full pedagogy prompt):
/// the benchmark measures NPU throughput, not Socratic quality, and a
/// shorter system prompt keeps prefill cost representative of the 4K
/// context budget rather than dominated by a multi-kilobyte instruction
/// block. A line that wants the production-shaped prompt can carry its own
/// `system`.
pub const DEFAULT_BENCH_SYSTEM_PROMPT: &str = "You are the Primer, a kind Socratic tutor for a curious child. Answer briefly, \
     then ask one short question that nudges the child to think further. Keep replies \
     to two or three sentences.";

/// One loaded benchmark prompt: a human label plus the assembled
/// [`Prompt`] ready to hand to `generate_stream`.
///
/// No `PartialEq`: [`Prompt`] does not implement it, and the tests assert
/// on individual fields rather than whole-value equality.
#[derive(Debug, Clone)]
pub struct BenchPrompt {
    /// Short identifier used in the per-prompt log line.
    pub label: String,
    /// The assembled prompt (system + conversation turns).
    pub prompt: Prompt,
}

/// JSONL line schema. `turns` reuses [`Message`] so role parsing and the
/// chat-template render path see exactly the same type the production
/// dialogue manager produces.
#[derive(Debug, Deserialize)]
struct BenchPromptSpec {
    label: String,
    #[serde(default)]
    system: Option<String>,
    turns: Vec<Message>,
}

/// Errors raised while loading the corpus.
#[derive(Debug, Error)]
pub enum BenchPromptError {
    /// The corpus file could not be read.
    #[error("could not read bench prompts at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// A line was not valid `BenchPromptSpec` JSON.
    #[error("bench prompt line {line}: invalid JSON: {source}")]
    Parse {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    /// A line parsed but failed a structural invariant (empty label, no
    /// turns, or a final turn the model can't respond to).
    #[error("bench prompt line {line} ({label}): {reason}")]
    Invalid {
        line: usize,
        label: String,
        reason: String,
    },
    /// The file parsed but contained no prompts.
    #[error("bench prompts file {path} contained no prompts")]
    Empty { path: PathBuf },
}

/// Parse JSONL corpus text into [`BenchPrompt`]s. Pure — no I/O.
///
/// Blank lines (and all-whitespace lines) are skipped so a trailing
/// newline or human spacing between entries is harmless. Line numbers in
/// errors are 1-based and count every physical line (including skipped
/// blanks) so they match what an editor shows.
pub fn parse_bench_prompts(
    jsonl: &str,
    default_system: &str,
) -> Result<Vec<BenchPrompt>, BenchPromptError> {
    let mut prompts = Vec::new();
    for (idx, raw) in jsonl.lines().enumerate() {
        let line = idx + 1;
        if raw.trim().is_empty() {
            continue;
        }
        let spec: BenchPromptSpec =
            serde_json::from_str(raw).map_err(|source| BenchPromptError::Parse { line, source })?;
        prompts.push(spec_to_prompt(spec, default_system, line)?);
    }
    Ok(prompts)
}

/// Validate one spec and assemble it into a [`BenchPrompt`].
fn spec_to_prompt(
    spec: BenchPromptSpec,
    default_system: &str,
    line: usize,
) -> Result<BenchPrompt, BenchPromptError> {
    let invalid = |reason: &str| BenchPromptError::Invalid {
        line,
        label: spec.label.clone(),
        reason: reason.to_string(),
    };
    if spec.label.trim().is_empty() {
        return Err(invalid("label must not be empty"));
    }
    if spec.turns.is_empty() {
        return Err(invalid("turns must not be empty"));
    }
    // The model is being asked to produce the *next* assistant turn, so the
    // conversation must end on a user turn — otherwise there's nothing to
    // respond to and the timing measurement is meaningless.
    if spec.turns.last().map(|m| m.role) != Some(Role::User) {
        return Err(invalid("the final turn must have role \"user\""));
    }
    let system = spec.system.unwrap_or_else(|| default_system.to_string());
    Ok(BenchPrompt {
        label: spec.label,
        prompt: Prompt {
            system,
            messages: spec.turns,
        },
    })
}

/// Read and parse the corpus file. Errors when the file is unreadable, any
/// line is malformed, or the file yields zero prompts.
pub fn load_bench_prompts(
    path: &Path,
    default_system: &str,
) -> Result<Vec<BenchPrompt>, BenchPromptError> {
    let text = std::fs::read_to_string(path).map_err(|source| BenchPromptError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let prompts = parse_bench_prompts(&text, default_system)?;
    if prompts.is_empty() {
        return Err(BenchPromptError::Empty {
            path: path.to_path_buf(),
        });
    }
    Ok(prompts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const SYS: &str = "default system";

    #[test]
    fn parses_single_user_turn_with_default_system() {
        let jsonl =
            r#"{"label":"sun","turns":[{"role":"user","content":"how does the sun shine"}]}"#;
        let prompts = parse_bench_prompts(jsonl, SYS).unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].label, "sun");
        assert_eq!(prompts[0].prompt.system, SYS);
        assert_eq!(prompts[0].prompt.messages.len(), 1);
        assert_eq!(
            prompts[0].prompt.messages[0].content,
            "how does the sun shine"
        );
        assert_eq!(prompts[0].prompt.messages[0].role, Role::User);
    }

    #[test]
    fn per_line_system_overrides_default() {
        let jsonl = r#"{"label":"x","system":"custom","turns":[{"role":"user","content":"hi"}]}"#;
        let prompts = parse_bench_prompts(jsonl, SYS).unwrap();
        assert_eq!(prompts[0].prompt.system, "custom");
    }

    #[test]
    fn parses_multi_turn_dialogue_continuation() {
        let jsonl = r#"{"label":"sun-react","turns":[{"role":"user","content":"how does the sun shine"},{"role":"assistant","content":"What do you think it is made of?"},{"role":"user","content":"i think fire"}]}"#;
        let prompts = parse_bench_prompts(jsonl, SYS).unwrap();
        assert_eq!(prompts[0].prompt.messages.len(), 3);
        assert_eq!(prompts[0].prompt.messages[2].role, Role::User);
    }

    #[test]
    fn skips_blank_lines() {
        let jsonl = "\n{\"label\":\"a\",\"turns\":[{\"role\":\"user\",\"content\":\"q\"}]}\n\n  \n";
        let prompts = parse_bench_prompts(jsonl, SYS).unwrap();
        assert_eq!(prompts.len(), 1);
    }

    #[test]
    fn malformed_json_reports_line_number() {
        // Line 1 blank, line 2 valid, line 3 broken.
        let jsonl =
            "\n{\"label\":\"a\",\"turns\":[{\"role\":\"user\",\"content\":\"q\"}]}\n{ not json";
        let err = parse_bench_prompts(jsonl, SYS).unwrap_err();
        match err {
            BenchPromptError::Parse { line, .. } => assert_eq!(line, 3),
            other => panic!("expected Parse; got {other:?}"),
        }
    }

    #[test]
    fn rejects_empty_label() {
        let jsonl = r#"{"label":"  ","turns":[{"role":"user","content":"q"}]}"#;
        let err = parse_bench_prompts(jsonl, SYS).unwrap_err();
        assert!(
            matches!(err, BenchPromptError::Invalid { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn rejects_empty_turns() {
        let jsonl = r#"{"label":"a","turns":[]}"#;
        let err = parse_bench_prompts(jsonl, SYS).unwrap_err();
        assert!(
            matches!(err, BenchPromptError::Invalid { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn rejects_final_turn_not_user() {
        let jsonl = r#"{"label":"a","turns":[{"role":"user","content":"q"},{"role":"assistant","content":"a"}]}"#;
        let err = parse_bench_prompts(jsonl, SYS).unwrap_err();
        match err {
            BenchPromptError::Invalid { reason, .. } => assert!(reason.contains("user")),
            other => panic!("expected Invalid; got {other:?}"),
        }
    }

    #[test]
    fn empty_corpus_parses_to_empty_vec() {
        // The pure parser tolerates an all-blank corpus; the file loader is
        // what rejects it (it has the path for the error).
        assert!(parse_bench_prompts("\n  \n", SYS).unwrap().is_empty());
    }

    #[test]
    fn load_reads_and_parses_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("prompts.jsonl");
        std::fs::write(
            &path,
            r#"{"label":"a","turns":[{"role":"user","content":"q"}]}"#,
        )
        .unwrap();
        let prompts = load_bench_prompts(&path, SYS).unwrap();
        assert_eq!(prompts.len(), 1);
    }

    #[test]
    fn load_errors_on_missing_file() {
        let err = load_bench_prompts(Path::new("/no/such/file.jsonl"), SYS).unwrap_err();
        assert!(matches!(err, BenchPromptError::Io { .. }), "got {err:?}");
    }

    #[test]
    fn load_errors_on_empty_corpus() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("empty.jsonl");
        std::fs::write(&path, "\n\n").unwrap();
        let err = load_bench_prompts(&path, SYS).unwrap_err();
        assert!(matches!(err, BenchPromptError::Empty { .. }), "got {err:?}");
    }
}
