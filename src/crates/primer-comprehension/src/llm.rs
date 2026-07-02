//! LLM-backed comprehension classifier. Wraps any `InferenceBackend`
//! (via `Arc<dyn ...>`) so the chat backend can be reused or a cheaper
//! comprehension-specific model can be swapped in independently.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::warn;

use primer_core::comprehension::{
    ComprehensionAssessment, ComprehensionContext, ComprehensionResult,
};
use primer_core::conversation::Speaker;
use primer_core::error::Result;
use primer_core::inference::{InferenceBackend, Message, Prompt, Role};
use primer_core::learner::UnderstandingDepth;
use primer_core::llm_util::{extract_first_json_object, normalize_concept_key, truncate_to_chars};

use crate::ComprehensionClassifier;
use crate::consts;
use crate::settings::ComprehensionSettings;

pub struct LlmComprehensionClassifier {
    backend: Arc<dyn InferenceBackend>,
    settings: ComprehensionSettings,
    identifier: String,
}

impl LlmComprehensionClassifier {
    /// Build a new LLM-backed comprehension classifier.
    ///
    /// The stable `identifier` is composed as `llm:{backend}:{model}`,
    /// matching the format used by `LlmConceptExtractor`.
    pub fn new(
        backend: Arc<dyn InferenceBackend>,
        model: String,
        settings: ComprehensionSettings,
    ) -> Self {
        let identifier = format!("llm:{}:{}", backend.name(), model);
        Self {
            backend,
            settings,
            identifier,
        }
    }
}

#[async_trait]
impl ComprehensionClassifier for LlmComprehensionClassifier {
    fn identifier(&self) -> &str {
        &self.identifier
    }

    async fn classify(&self, ctx: ComprehensionContext<'_>) -> Result<ComprehensionResult> {
        if ctx.candidate_concepts.is_empty() {
            return Ok(ComprehensionResult::empty());
        }

        let prompt = build_classification_prompt(&ctx);
        let params = primer_core::inference::GenerationParams {
            max_tokens: self.settings.generation_max_tokens,
            temperature: self.settings.generation_temperature,
            top_p: self.settings.generation_top_p,
            stop_sequences: vec![],
            routing: None,
        };
        let raw = match self.backend.generate(&prompt, &params).await {
            Ok(r) => r,
            Err(e) => {
                warn!(classifier = %self.identifier, error = ?e, "comprehension backend call failed");
                return Ok(ComprehensionResult::empty());
            }
        };

        let truncated = truncate_to_chars(&raw, self.settings.max_output_chars);
        match parse_classification_output(
            truncated,
            ctx.candidate_concepts,
            self.settings.evidence_max_chars,
            self.settings.confidence_threshold,
        ) {
            Ok(r) => Ok(r),
            Err(reason) => {
                let snippet: String = truncated
                    .chars()
                    .take(consts::LLM_DEBUG_SNIPPET_CHARS)
                    .collect();
                warn!(classifier = %self.identifier, raw = %snippet, %reason, "comprehension output unparseable");
                Ok(ComprehensionResult::empty())
            }
        }
    }
}

// ─── Prompt builder ───────────────────────────────────────────────────────────

/// Literal example JSON embedded in the classification prompt. Kept as
/// a const so the parser can excise a verbatim echo of it from the
/// model output before extracting the first JSON object — small local
/// models sometimes restate the example before answering, and
/// first-object-wins parsing would otherwise adopt the example as a
/// real assessment whenever "gravity" happens to be a candidate.
const EXAMPLE_OUTPUT: &str = r#"{"assessments": [{"concept": "gravity", "depth": "Comprehension", "confidence": 0.8, "evidence": "Explained in their own words that heavier things do not fall faster."}]}"#;

fn build_classification_prompt(ctx: &ComprehensionContext) -> Prompt {
    let context_section = if ctx.recent_turns.is_empty() {
        "(no prior context)".to_string()
    } else {
        ctx.recent_turns
            .iter()
            .map(|t| format!("{}: {}", speaker_label(t.speaker), t.text))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let candidates_section = ctx
        .candidate_concepts
        .iter()
        .map(|c| format!("- {c}"))
        .collect::<Vec<_>>()
        .join("\n");

    // The depth ladder offered to the model deliberately excludes
    // `Unknown`: a small model told "use Unknown when unsure" emits
    // noise rows instead of omitting. The parser still accepts
    // "Unknown" defensively (so the row is preserved for the
    // longitudinal record), but the apply step skips Unknown rows
    // entirely — monotonic max alone would protect depth, yet the
    // Leitner-box transition would otherwise treat a high-confidence
    // Unknown as a success and advance the box off a no-evidence
    // reading. The prompt only ever asks for the five real levels.
    let depth_levels = UnderstandingDepth::ALL
        .iter()
        .filter(|d| **d != UnderstandingDepth::Unknown)
        .map(|d| d.name())
        .collect::<Vec<_>>()
        .join(", ");

    // Written for small local models: one-line rules, a literal example
    // output, and the JSON-only instruction repeated at the end of the
    // user message where recency keeps it salient.
    let system = format!(
        "You assess how deeply a child understands specific concepts, based on what the \
        child actually said in a learning conversation.\n\n\
        Depth levels (shallowest to deepest):\n\
        - Aware: the concept registered — the child named it back or asked about it.\n\
        - Recall: the child stated a fact about it.\n\
        - Comprehension: the child explained it in their own words.\n\
        - Application: the child applied it to a new situation.\n\
        - Analysis: the child compared, contrasted, or reasoned about it.\n\n\
        Rules:\n\
        - Judge ONLY from the child's own words. What the Primer said is not evidence.\n\
        - Copy each concept name EXACTLY as it appears in the candidate list.\n\
        - \"depth\" is exactly one of: {depth_levels}.\n\
        - \"confidence\" is a number between 0.0 and 1.0.\n\
        - \"evidence\" is one short sentence quoting or paraphrasing what the child said.\n\
        - If the child did not engage with a concept, or only said \"yeah\"/\"ok\"/\"I see\", \
        leave that concept out of the list. When unsure, leave it out.\n\
        - It is fine to return {{\"assessments\": []}}.\n\
        - Output ONLY one JSON object. No other text, no markdown fences.\n\n\
        Example output:\n\
        {EXAMPLE_OUTPUT}"
    );

    let user = format!(
        "Prior context (oldest first):\n{context_section}\n\nExchange to analyse:\n{}: {}\n{}: {}\n\nCandidate concepts to assess (only these — do not invent others):\n{candidates_section}\n\nAssess now. Answer with the JSON object only.",
        speaker_label(ctx.child_turn.speaker),
        ctx.child_turn.text,
        speaker_label(ctx.primer_turn.speaker),
        ctx.primer_turn.text,
    );

    Prompt {
        system,
        messages: vec![Message {
            role: Role::User,
            content: user,
        }],
    }
}

fn speaker_label(s: Speaker) -> &'static str {
    match s {
        Speaker::Child => "Child",
        Speaker::Primer => "Primer",
    }
}

// ─── Output parser ─────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct ClassifierOutput {
    #[serde(default)]
    assessments: Vec<RawAssessment>,
}

#[derive(serde::Deserialize)]
struct RawAssessment {
    concept: String,
    depth: String,
    confidence: f32,
    #[serde(default)]
    evidence: Option<String>,
}

fn parse_classification_output(
    raw: &str,
    candidates: &[String],
    evidence_max_chars: usize,
    confidence_threshold: f32,
) -> std::result::Result<ComprehensionResult, String> {
    // Excise any verbatim echo of the prompt's example before extracting:
    // with first-object-wins parsing, an echoed example would otherwise
    // be adopted as a real assessment whenever "gravity" is a candidate.
    // If nothing parseable remains, the model's entire output WAS the
    // example — indistinguishable from a genuine identical answer — so
    // fall back to the raw output rather than soft-failing.
    let cleaned = raw.replace(EXAMPLE_OUTPUT, "");
    let json_str = match extract_first_json_object(&cleaned) {
        Some(s) => s,
        None => extract_first_json_object(raw)
            .ok_or_else(|| "no JSON object found in output".to_string())?,
    };
    let parsed: ClassifierOutput =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {e}"))?;

    // Normalize the candidate list once (not per assessment). The
    // shared `normalize_concept_key` is the same rule the extractor
    // applies when producing candidates, so matching stays in lockstep
    // with the producer.
    let normalized_candidates: Vec<(String, &String)> = candidates
        .iter()
        .map(|c| (normalize_concept_key(c), c))
        .collect();

    let mut out: Vec<ComprehensionAssessment> = Vec::with_capacity(parsed.assessments.len());
    let mut index_by_concept: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for a in parsed.assessments {
        // Match the concept against the candidate list (defends against
        // the LLM inventing concepts). Matching is trimmed and
        // case-insensitive — small local models routinely echo
        // "Photosynthesis" for the candidate "photosynthesis" — but the
        // *canonical* candidate string is what lands in the assessment,
        // so downstream concept ids stay consistent with the extractor's
        // normalized form.
        let concept_returned = normalize_concept_key(&a.concept);
        let canonical = match normalized_candidates
            .iter()
            .find(|(n, _)| *n == concept_returned)
        {
            Some((_, c)) => (*c).clone(),
            None => continue,
        };
        // Drop assessments with out-of-range confidence.
        if !(0.0..=1.0).contains(&a.confidence) {
            continue;
        }
        // Drop assessments with unknown depth name.
        let depth = match depth_from_name(&a.depth) {
            Some(d) => d,
            None => continue,
        };
        let evidence = a.evidence.map(|e| {
            let trimmed = truncate_to_chars(&e, evidence_max_chars);
            trimmed.to_string()
        });
        let assessment = ComprehensionAssessment {
            concept: canonical,
            depth,
            confidence: a.confidence,
            evidence,
        };
        // At most ONE assessment per canonical concept. Duplicate rows
        // (e.g. the model returns both "Gravity" and "gravity") would
        // each run the Leitner-box transition downstream, advancing the
        // box twice off a single exchange. Keep the strongest reading:
        // a row that will actually apply (confidence >= threshold)
        // always beats one that won't — otherwise a sub-threshold
        // Analysis row could displace an above-threshold Recall row and
        // silently drop the turn's learner update — then deepest depth,
        // then highest confidence (the monotonic-max spirit of apply).
        match index_by_concept.get(&assessment.concept) {
            None => {
                index_by_concept.insert(assessment.concept.clone(), out.len());
                out.push(assessment);
            }
            Some(&i) => {
                let existing = &mut out[i];
                let new_applies = assessment.confidence >= confidence_threshold;
                let old_applies = existing.confidence >= confidence_threshold;
                let stronger = (new_applies && !old_applies)
                    || (new_applies == old_applies
                        && (assessment.depth > existing.depth
                            || (assessment.depth == existing.depth
                                && assessment.confidence > existing.confidence)));
                if stronger {
                    *existing = assessment;
                }
            }
        }
    }

    Ok(ComprehensionResult { assessments: out })
}

fn depth_from_name(name: &str) -> Option<UnderstandingDepth> {
    UnderstandingDepth::ALL
        .iter()
        .copied()
        .find(|d| d.name() == name)
}

// ─── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
