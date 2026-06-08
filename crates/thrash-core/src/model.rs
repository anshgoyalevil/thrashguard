//! Data model for the agent's observable behaviour.
//!
//! ThrashGuard does not see the model's hidden reasoning — only what crosses
//! the wire: which files the agent wrote, which it read, and what result it
//! observed afterwards. A [`Turn`] is one such observable step.

use serde::{Deserialize, Serialize};

/// One observable agent step: what it changed, what it inspected, and the
/// result it then observed (a test run, a compile, a command exit).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    /// Files written or edited during this turn.
    #[serde(default)]
    pub edits: Vec<FileEdit>,
    /// Files merely read/inspected during this turn (paths only).
    #[serde(default)]
    pub reads: Vec<String>,
    /// The result the agent observed after acting.
    pub observation: Observation,
    /// Optional human-readable label, used by the demo and logs.
    #[serde(default)]
    pub note: Option<String>,
}

impl Turn {
    /// Convenience constructor for a single-file edit followed by an error.
    pub fn edit_then_error(
        path: impl Into<String>,
        content: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Turn {
            edits: vec![FileEdit {
                path: path.into(),
                content: content.into(),
            }],
            reads: Vec::new(),
            observation: Observation::error(error),
            note: None,
        }
    }
}

/// A single file write. `content` is the full post-edit file body (or the
/// replacement fragment) — whatever the proxy can recover from the tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEdit {
    pub path: String,
    pub content: String,
}

/// The result the agent observed after acting on a turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub kind: ObservationKind,
    /// Normalised signature of the result — e.g. the error class plus the
    /// salient line of the message. Two turns with the same signature are
    /// considered to have produced "the same outcome".
    #[serde(default)]
    pub signature: String,
}

impl Observation {
    pub fn error(signature: impl Into<String>) -> Self {
        Observation {
            kind: ObservationKind::Error,
            signature: signature.into(),
        }
    }
    pub fn success() -> Self {
        Observation {
            kind: ObservationKind::Success,
            signature: String::new(),
        }
    }
    pub fn neutral(signature: impl Into<String>) -> Self {
        Observation {
            kind: ObservationKind::Neutral,
            signature: signature.into(),
        }
    }
}

/// Coarse classification of an observed outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationKind {
    /// A failure: test failure, compile error, non-zero exit, panic.
    Error,
    /// A clean result: tests pass, build succeeds.
    Success,
    /// Neither clearly good nor bad (informational command output).
    Neutral,
}

impl ObservationKind {
    pub fn is_error(self) -> bool {
        matches!(self, ObservationKind::Error)
    }
}
