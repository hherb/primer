//! Off-topic utterances for the hybrid off-topic gate (issue #307).
//!
//! Small talk a child says to the Primer that no knowledge passage should
//! answer: greetings, meta-talk ("can we stop now"), feelings. Under the
//! production gate (`kb_query_gate_min_cosine`) hybrid retrieval should
//! return nothing for these.

/// English off-topic utterances.
pub const OFF_TOPIC_EN: &[&str] = &[
    "good morning",
    "hello",
    "hi there",
    "can we stop now",
    "i am bored",
    "i don't want to talk anymore",
    "thank you",
    "okay",
    "yes",
    "no",
    "bye",
    "what is your name",
    "that is funny",
    "can we play a game",
    "i like you",
    "see you tomorrow",
    "hmm",
    "i don't know",
];

/// German off-topic utterances.
pub const OFF_TOPIC_DE: &[&str] = &[
    "guten Morgen",
    "hallo",
    "können wir jetzt aufhören",
    "mir ist langweilig",
    "danke",
    "ja",
    "nein",
    "tschüss",
    "wie heißt du",
    "das ist lustig",
    "ich habe keine Lust mehr",
    "können wir ein Spiel spielen",
    "bis morgen",
    "weiß ich nicht",
    "okay",
];

/// Off-topic utterances whose best BGE-M3 cosine (0.451–0.473) clears the
/// 0.45 gate, so they still retrieve passages. The gate leaves a recall
/// margin below the lowest on-topic benchmark query (0.473) rather than
/// catch these; see `KB_QUERY_GATE_MIN_COSINE_BGE_M3`.
///
/// Keyed by text alone, not locale: an utterance in both lists (e.g.
/// "okay") must leak in both locales or neither, or the both-ways check
/// can't be satisfied — reword one copy if they ever diverge.
pub const KNOWN_LEAKING_OFF_TOPIC: &[&str] = &[
    "good morning",
    "hello",
    "i am bored",
    "hmm",
    "i don't know",
    "can we play a game",
    "mir ist langweilig",
];
