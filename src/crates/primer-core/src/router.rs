//! Phase 1.3 inference-router policy: pure, I/O-free decision logic shared by
//! the wiring layer (which constructs the router) and the router decorator in
//! `primer-inference`. Kept in `primer-core` so it carries no inference
//! dependency and is unit-testable on the default `cargo test`.
//!
//! See docs/superpowers/specs/2026-06-07-inference-router-design.md.

use std::str::FromStr;

/// How the router chooses between the primary (typically local/small) and
/// secondary (typically cloud/strong) legs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum RouterMode {
    /// Never use the secondary leg. The runtime works with zero network.
    /// This is the privacy default.
    #[default]
    LocalOnly,
    /// Always try the secondary first; fall back to the primary on a
    /// pre-stream failure.
    CloudPreferred,
    /// Score each turn; route high-complexity turns to the secondary, routine
    /// turns to the primary. Either leg covers the other on pre-stream failure.
    Hybrid,
}

impl RouterMode {
    /// Every variant, in declaration order (for CLI help / GUI pickers).
    pub const ALL: &'static [Self] = &[Self::LocalOnly, Self::CloudPreferred, Self::Hybrid];

    /// Canonical kebab-case name. Stable identifier used by CLI flags, the
    /// GUI picker values, and config serialization — do not rename.
    pub fn name(self) -> &'static str {
        match self {
            Self::LocalOnly => "local-only",
            Self::CloudPreferred => "cloud-preferred",
            Self::Hybrid => "hybrid",
        }
    }

    /// True when this mode may route to the secondary leg (i.e. NOT local-only).
    pub fn uses_secondary(self) -> bool {
        !matches!(self, Self::LocalOnly)
    }
}

impl std::fmt::Display for RouterMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for RouterMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "local-only" => Ok(Self::LocalOnly),
            "cloud-preferred" => Ok(Self::CloudPreferred),
            "hybrid" => Ok(Self::Hybrid),
            other => Err(format!(
                "unknown router mode '{other}' (expected local-only, cloud-preferred, or hybrid)"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_local_only() {
        assert_eq!(RouterMode::default(), RouterMode::LocalOnly);
    }

    #[test]
    fn name_and_from_str_round_trip() {
        for &m in RouterMode::ALL {
            assert_eq!(RouterMode::from_str(m.name()).unwrap(), m);
        }
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!(RouterMode::from_str("nonsense").is_err());
    }

    #[test]
    fn only_local_only_skips_secondary() {
        assert!(!RouterMode::LocalOnly.uses_secondary());
        assert!(RouterMode::CloudPreferred.uses_secondary());
        assert!(RouterMode::Hybrid.uses_secondary());
    }
}
