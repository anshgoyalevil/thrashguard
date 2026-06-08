//! The thrash detector: a streaming state machine over agent behaviour.
//!
//! # Model
//!
//! Each [`Turn`] the agent edits some files and then observes a result. For
//! every edited file we keep its history of *versions* — a `(fingerprint,
//! content, outcome)` triple per write. A file is "thrashing" when the agent
//! keeps returning it to a state it has already been in **and** that state
//! keeps producing the same failing outcome.
//!
//! # Detection
//!
//! For each edit we resolve a *canonical state id*: its fingerprint, unless a
//! recent version is near-identical (Levenshtein similarity above the policy
//! threshold), in which case we reuse that version's id. This folds cosmetic
//! churn into one logical state.
//!
//! We then count occurrences of the key `(path, canonical_state, error)`. The
//! first occurrence is normal exploration. Each subsequent occurrence is the
//! agent re-entering a known-bad state — i.e. a cycle. We **warn** at
//! `policy.warn_at` and **trip** at `policy.trip_at`.
//!
//! This is deliberately explainable (no opaque ML): every trip can be traced to
//! a concrete list of turns that produced the identical state and error.

use std::collections::HashMap;

use crate::fingerprint::{fingerprint, similarity};
use crate::intervention::Intervention;
use crate::model::{ObservationKind, Turn};
use crate::policy::ThrashPolicy;

/// Severity of a single turn's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Nothing notable — normal forward progress or exploration.
    Ok,
    /// A repeated state/error was seen; the agent may be circling.
    Warn,
    /// A confirmed thrash cycle crossed the trip threshold.
    Trip,
}

/// A per-file finding produced while ingesting a turn.
#[derive(Debug, Clone)]
pub struct Signal {
    pub path: String,
    /// How many times this `(state, error)` pair has now occurred.
    pub occurrences: u32,
    pub observation: ObservationKind,
    /// The earliest turn that produced this same state, if it is a repeat.
    pub first_seen_turn: Option<usize>,
    /// Distinct versions the file has cycled between.
    pub distinct_states: usize,
    /// Percent of the file that changed versus its immediately previous
    /// version, in `[0.0, 1.0]` (0 == identical, 1 == fully rewritten).
    pub change_ratio: f64,
    pub severity: Severity,
    pub message: String,
}

/// The verdict for one ingested turn.
#[derive(Debug, Clone)]
pub struct Verdict {
    pub turn_index: usize,
    pub severity: Severity,
    pub signals: Vec<Signal>,
    /// Present iff `severity == Trip`.
    pub intervention: Option<Intervention>,
}

impl Verdict {
    pub fn tripped(&self) -> bool {
        self.severity == Severity::Trip
    }
}

#[derive(Debug, Clone)]
struct Version {
    canonical: u64,
    content: String,
}

#[derive(Debug, Default, Clone)]
struct Occurrence {
    count: u32,
    first_turn: usize,
}

/// Streaming detector. Feed it [`Turn`]s in order via [`ingest`]; it returns a
/// [`Verdict`] per turn and trips at most once per `(file, state, error)`.
///
/// [`ingest`]: ThrashDetector::ingest
#[derive(Debug, Clone)]
pub struct ThrashDetector {
    policy: ThrashPolicy,
    turn_index: usize,
    /// Per-file version history.
    versions: HashMap<String, Vec<Version>>,
    /// Occurrence counts keyed by `(path, canonical_state, error_signature)`.
    occurrences: HashMap<(String, u64, String), Occurrence>,
    /// Files read but never edited, with read counts — intervention targets.
    read_only: HashMap<String, u32>,
    edited: std::collections::HashSet<String>,
    /// Keys we have already tripped on, so we don't re-trip every turn.
    tripped_keys: std::collections::HashSet<(String, u64, String)>,
}

impl ThrashDetector {
    pub fn new(policy: ThrashPolicy) -> Self {
        Self {
            policy,
            turn_index: 0,
            versions: HashMap::new(),
            occurrences: HashMap::new(),
            read_only: HashMap::new(),
            edited: std::collections::HashSet::new(),
            tripped_keys: std::collections::HashSet::new(),
        }
    }

    pub fn with_default_policy() -> Self {
        Self::new(ThrashPolicy::default())
    }

    pub fn policy(&self) -> &ThrashPolicy {
        &self.policy
    }

    /// Ingest one turn and return its verdict.
    pub fn ingest(&mut self, turn: &Turn) -> Verdict {
        let idx = self.turn_index;
        self.turn_index += 1;

        // Track reads (only the ones we've never edited count as redirect
        // candidates; an edited file is presumably already in play).
        for r in &turn.reads {
            if !self.edited.contains(r) {
                *self.read_only.entry(r.clone()).or_insert(0) += 1;
            }
        }

        let obs = &turn.observation;
        let mut signals = Vec::new();

        for edit in &turn.edits {
            self.edited.insert(edit.path.clone());
            self.read_only.remove(&edit.path);

            let fp = fingerprint(&edit.content);
            let history = self.versions.entry(edit.path.clone()).or_default();

            // Change ratio vs the immediately previous version of this file.
            let change_ratio = match history.last() {
                Some(prev) => 1.0 - similarity(&prev.content, &edit.content),
                None => 1.0,
            };

            // Resolve the canonical state id: reuse a near-identical recent
            // version's id, else this fingerprint becomes a new state id.
            let canonical = self.resolve_canonical(&edit.path, fp, &edit.content);

            self.versions
                .get_mut(&edit.path)
                .expect("just inserted")
                .push(Version {
                    canonical,
                    content: edit.content.clone(),
                });

            let distinct_states = {
                let mut s: Vec<u64> = self.versions[&edit.path]
                    .iter()
                    .map(|v| v.canonical)
                    .collect();
                s.sort_unstable();
                s.dedup();
                s.len()
            };

            // Occurrence counting is keyed on the *outcome* too: re-entering a
            // state that now passes is fine; re-entering one that keeps failing
            // is the thrash.
            let key = (edit.path.clone(), canonical, obs.signature.clone());
            let entry = self.occurrences.entry(key.clone()).or_insert(Occurrence {
                count: 0,
                first_turn: idx,
            });
            entry.count += 1;
            let occurrences = entry.count;
            let first_turn = entry.first_turn;

            let severity = self.classify(obs.kind, occurrences, &key);

            let first_seen_turn = (occurrences > 1).then_some(first_turn);
            let message = render_signal_message(
                &edit.path,
                obs.kind,
                occurrences,
                first_seen_turn,
                change_ratio,
            );

            signals.push(Signal {
                path: edit.path.clone(),
                occurrences,
                observation: obs.kind,
                first_seen_turn,
                distinct_states,
                change_ratio,
                severity,
                message,
            });
        }

        self.assemble_verdict(idx, signals, obs)
    }

    fn classify(
        &mut self,
        kind: ObservationKind,
        occurrences: u32,
        key: &(String, u64, String),
    ) -> Severity {
        // Only failing outcomes can thrash — re-reaching a passing state is
        // convergence, not a loop.
        if !kind.is_error() {
            return Severity::Ok;
        }
        if occurrences >= self.policy.trip_at && !self.tripped_keys.contains(key) {
            self.tripped_keys.insert(key.clone());
            Severity::Trip
        } else if occurrences >= self.policy.warn_at {
            Severity::Warn
        } else {
            Severity::Ok
        }
    }

    fn assemble_verdict(
        &self,
        idx: usize,
        signals: Vec<Signal>,
        obs: &crate::model::Observation,
    ) -> Verdict {
        let severity = signals
            .iter()
            .map(|s| s.severity)
            .max_by_key(|s| match s {
                Severity::Ok => 0,
                Severity::Warn => 1,
                Severity::Trip => 2,
            })
            .unwrap_or(Severity::Ok);

        let intervention = if severity == Severity::Trip {
            let tripped = signals
                .iter()
                .find(|s| s.severity == Severity::Trip)
                .expect("trip severity implies a tripped signal");
            Some(Intervention::build(
                &tripped.path,
                &obs.signature,
                tripped.occurrences,
                tripped.distinct_states,
                self.suggest_redirect(&tripped.path),
            ))
        } else {
            None
        };

        Verdict {
            turn_index: idx,
            severity,
            signals,
            intervention,
        }
    }

    /// Resolve a canonical state id for `content`: if a recent version (within
    /// `policy.lookback`) is an exact or near-duplicate, reuse its id.
    fn resolve_canonical(&self, path: &str, fp: u64, content: &str) -> u64 {
        let Some(history) = self.versions.get(path) else {
            return fp;
        };
        let start = history.len().saturating_sub(self.policy.lookback);
        for v in history[start..].iter().rev() {
            if v.canonical == fp {
                return v.canonical;
            }
            if similarity(&v.content, content) >= self.policy.similarity_threshold {
                return v.canonical;
            }
        }
        fp
    }

    /// Choose a file to redirect attention to: the most-read file that was
    /// never edited, preferring one other than the thrashed file.
    fn suggest_redirect(&self, thrashed: &str) -> Option<String> {
        self.read_only
            .iter()
            .filter(|(path, _)| path.as_str() != thrashed)
            .max_by_key(|(_, count)| **count)
            .map(|(path, _)| path.clone())
    }
}

fn render_signal_message(
    path: &str,
    kind: ObservationKind,
    occurrences: u32,
    first_seen_turn: Option<usize>,
    change_ratio: f64,
) -> String {
    match (kind, first_seen_turn) {
        (ObservationKind::Error, Some(first)) => format!(
            "`{path}` returned to a prior failing state (first seen on turn {first}); \
             occurrence #{occurrences}, {pct:.0}% changed vs last write",
            pct = change_ratio * 100.0,
        ),
        (ObservationKind::Success, _) => {
            format!(
                "`{path}` reached a passing state ({pct:.0}% changed)",
                pct = change_ratio * 100.0
            )
        }
        _ => format!(
            "`{path}` edited ({pct:.0}% changed vs last write)",
            pct = change_ratio * 100.0
        ),
    }
}
