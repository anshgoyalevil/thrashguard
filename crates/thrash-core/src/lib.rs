//! # thrash-core
//!
//! The behavioural loop-detection engine behind ThrashGuard — a circuit breaker
//! for AI coding agents stuck in "hallucination loops" (edit a file, run a
//! test, see the same error, revert, repeat — burning budget indefinitely).
//!
//! The engine is transport-agnostic: it consumes a stream of observable
//! [`Turn`]s (file edits, reads, and the outcome the agent observed) and emits
//! a [`Verdict`] per turn. When it detects a confirmed cycle it produces an
//! [`Intervention`] — a trusted operator note designed to be injected into the
//! conversation to break the loop.
//!
//! ```
//! use thrash_core::{ThrashDetector, Turn, Severity};
//!
//! let mut det = ThrashDetector::with_default_policy();
//!
//! // Agent oscillates `auth.ts` between two states, same error each time.
//! let a = "function token(s){ return s.token }";
//! let b = "function token(s){ if (!s) return null; return s.token }";
//! let err = "TypeError: cannot read 'token' of undefined";
//!
//! let mut last = None;
//! for content in [a, b, a, b, a] {
//!     last = Some(det.ingest(&Turn::edit_then_error("auth.ts", content, err)));
//! }
//! let verdict = last.unwrap();
//! assert_eq!(verdict.severity, Severity::Trip);
//! assert!(verdict.intervention.is_some());
//! ```

mod detector;
mod fingerprint;
mod intervention;
mod model;
mod policy;

pub use detector::{Severity, Signal, ThrashDetector, Verdict};
pub use fingerprint::{fingerprint, levenshtein, similarity};
pub use intervention::Intervention;
pub use model::{FileEdit, Observation, ObservationKind, Turn};
pub use policy::ThrashPolicy;

/// A loadable demo/replay scenario: a labelled sequence of turns.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Scenario {
    pub scenario: String,
    #[serde(default)]
    pub description: String,
    pub turns: Vec<Turn>,
}

impl Scenario {
    /// Parse a scenario from JSON.
    pub fn from_json(s: &str) -> serde_json::Result<Self> {
        serde_json::from_str(s)
    }
}
