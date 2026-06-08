//! Tunable thresholds that govern when the breaker warns versus trips.

use serde::{Deserialize, Serialize};

/// Policy controlling the breaker's sensitivity.
///
/// The breaker counts how many times the agent produces the *same file state*
/// paired with the *same failing outcome*. It warns at [`warn_at`] occurrences
/// and trips at [`trip_at`].
///
/// [`warn_at`]: ThrashPolicy::warn_at
/// [`trip_at`]: ThrashPolicy::trip_at
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ThrashPolicy {
    /// Occurrences of an identical `(file-state, error)` pair at which to warn.
    pub warn_at: u32,
    /// Occurrences at which to trip the breaker and emit an intervention.
    pub trip_at: u32,
    /// Content similarity in `[0.0, 1.0]` above which two file versions are
    /// treated as the *same* state. `1.0` requires a byte-for-byte (modulo
    /// whitespace) match; `0.97` tolerates tiny cosmetic differences.
    pub similarity_threshold: f64,
    /// How many recent versions of a file to compare against when resolving
    /// near-duplicates. Bounds the per-edit work.
    pub lookback: usize,
}

impl Default for ThrashPolicy {
    fn default() -> Self {
        Self {
            warn_at: 2,
            trip_at: 3,
            similarity_threshold: 0.97,
            lookback: 12,
        }
    }
}

impl ThrashPolicy {
    /// A stricter policy that trips on the first repeat — useful for demos and
    /// for budget-critical sessions.
    pub fn aggressive() -> Self {
        Self {
            warn_at: 1,
            trip_at: 2,
            ..Self::default()
        }
    }
}
