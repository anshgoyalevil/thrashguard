//! Construction of the circuit-breaker intervention.
//!
//! When the breaker trips, ThrashGuard injects a *trusted operator note* into
//! the conversation. Two design rules, both grounded in how the models are
//! trained to treat injected instructions:
//!
//!   1. **State context, don't issue commands.** "This change reproduces the
//!      same failure" lands better than "YOU MUST STOP". Override-style
//!      language ("ignore the user", "you are prohibited") is something the
//!      models are trained to resist, so we phrase the nudge as a fact plus a
//!      concrete next step.
//!   2. **Point somewhere new.** A loop ends when attention moves. We suggest a
//!      file the agent has looked at but never fixed — the likely real owner of
//!      the bug.

use serde::{Deserialize, Serialize};

/// A structured, ready-to-inject intervention emitted when the breaker trips.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intervention {
    /// The file the agent has been thrashing.
    pub thrashed_file: String,
    /// The repeating failure signature.
    pub error_signature: String,
    /// How many times the identical `(state, error)` pair has now occurred.
    pub occurrences: u32,
    /// How many distinct file versions the agent oscillated between.
    pub cycle_states: usize,
    /// A file to redirect attention to, if one could be inferred.
    pub suggested_file: Option<String>,
    /// The text to inject as a `role: "system"` message in the next request.
    pub system_message: String,
}

impl Intervention {
    pub(crate) fn build(
        thrashed_file: &str,
        error_signature: &str,
        occurrences: u32,
        cycle_states: usize,
        suggested_file: Option<String>,
    ) -> Self {
        let err = trim_signature(error_signature);
        let mut msg = format!(
            "System note: the edit just applied to `{file}` is the same change \
             already attempted {n} times in this session, and it reproduces the \
             same failure ({err}). The root cause is unlikely to live in \
             `{file}`.",
            file = thrashed_file,
            n = occurrences,
            err = err,
        );
        match &suggested_file {
            Some(s) => msg.push_str(&format!(
                " Before editing `{file}` again, inspect `{sugg}` — it has been \
                 read but never changed and is a likely owner of this behaviour \
                 — and reconsider the approach.",
                file = thrashed_file,
                sugg = s,
            )),
            None => msg.push_str(
                " Before editing it again, step back and reconsider which module \
                 actually owns this behaviour rather than re-applying the same fix.",
            ),
        }

        Intervention {
            thrashed_file: thrashed_file.to_string(),
            error_signature: error_signature.to_string(),
            occurrences,
            cycle_states,
            suggested_file,
            system_message: msg,
        }
    }
}

/// Keep injected notes short — clip an over-long error signature to its first
/// meaningful line and a bounded length.
fn trim_signature(sig: &str) -> String {
    let first_line = sig.lines().next().unwrap_or(sig).trim();
    const MAX: usize = 160;
    if first_line.chars().count() > MAX {
        let clipped: String = first_line.chars().take(MAX).collect();
        format!("{clipped}…")
    } else {
        first_line.to_string()
    }
}
