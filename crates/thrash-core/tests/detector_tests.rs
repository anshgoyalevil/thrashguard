//! Behavioural tests for the thrash detector.

use thrash_core::{Observation, Severity, ThrashDetector, ThrashPolicy, Turn};

const A: &str = "function token(session) { return session.token; }";
const B: &str = "function token(session) { return session && session.token; }";
const ERR: &str = "TypeError: cannot read property 'token' of undefined";

fn edit(path: &str, content: &str, reads: &[&str], obs: Observation) -> Turn {
    Turn {
        edits: vec![thrash_core::FileEdit {
            path: path.into(),
            content: content.into(),
        }],
        reads: reads.iter().map(|s| s.to_string()).collect(),
        observation: obs,
        note: None,
    }
}

#[test]
fn classic_aba_thrash_trips_at_default_threshold() {
    let mut det = ThrashDetector::with_default_policy(); // trip_at = 3
    let seq = [A, B, A, B, A];
    let mut verdicts = Vec::new();
    for c in seq {
        verdicts.push(det.ingest(&Turn::edit_then_error("auth.ts", c, ERR)));
    }

    // State A with ERR occurs on turns 0, 2, 4 -> third occurrence trips.
    assert_eq!(
        verdicts[0].severity,
        Severity::Ok,
        "first sight of A is fine"
    );
    assert_eq!(verdicts[2].severity, Severity::Warn, "second A warns");
    assert_eq!(verdicts[4].severity, Severity::Trip, "third A trips");

    let interv = verdicts[4]
        .intervention
        .as_ref()
        .expect("trip yields intervention");
    assert_eq!(interv.thrashed_file, "auth.ts");
    assert_eq!(interv.occurrences, 3);
    assert!(interv.system_message.contains("auth.ts"));
}

#[test]
fn only_trips_once_per_state() {
    let mut det = ThrashDetector::with_default_policy();
    // Only state A recurs to threshold; B/C/D are one-offs. A appears on turns
    // 0,2,4,6 — it must trip exactly once (at occurrence 3) and not re-trip on
    // occurrence 4.
    const C: &str = "function token(s) { return (s||{}).token; }";
    const D: &str = "function token(s) { return s?.token ?? null; }";
    let mut trips = 0;
    for c in [A, B, A, C, A, D, A] {
        if det
            .ingest(&Turn::edit_then_error("auth.ts", c, ERR))
            .tripped()
        {
            trips += 1;
        }
    }
    assert_eq!(trips, 1, "a given (state,error) trips at most once");
}

#[test]
fn genuine_progress_never_trips() {
    let mut det = ThrashDetector::with_default_policy();
    // Each turn is a real change ending in a *different* error, then success.
    let steps = [
        edit(
            "config.ts",
            "v1 = read()",
            &[],
            Observation::error("E1: missing key"),
        ),
        edit(
            "config.ts",
            "v2 = read().trim()",
            &[],
            Observation::error("E2: bad format"),
        ),
        edit(
            "config.ts",
            "v3 = parse(read().trim())",
            &[],
            Observation::success(),
        ),
    ];
    for t in steps {
        let v = det.ingest(&t);
        assert_ne!(v.severity, Severity::Trip, "forward progress must not trip");
    }
}

#[test]
fn reaching_a_passing_state_is_not_a_loop() {
    let mut det = ThrashDetector::with_default_policy();
    // Same content repeatedly, but it PASSES — not a thrash.
    let mut tripped = false;
    for _ in 0..5 {
        tripped |= det
            .ingest(&edit("ok.ts", "stable()", &[], Observation::success()))
            .tripped();
    }
    assert!(
        !tripped,
        "a repeated passing state is convergence, not thrash"
    );
}

#[test]
fn near_duplicate_edits_count_as_same_state() {
    let mut det = ThrashDetector::with_default_policy();
    // Two versions of a substantial file that differ only by a one-character
    // trailing comment tag (cosmetic). At >97% similarity they must fold into
    // one logical state, so the oscillation still trips.
    let base = "export function getToken(session) {\n  if (!session) { return null; }\n  const t = session.token;\n  return t;\n}\n";
    let a1 = format!("{base}// attempt tagged A");
    let a2 = format!("{base}// attempt tagged B"); // 1-char diff vs a1
    let b = "export function getToken(session) {\n  return session?.token ?? null;\n}";
    let seq = [a1.as_str(), b, a2.as_str(), b, a1.as_str()];
    let mut last = None;
    for c in seq {
        last = Some(det.ingest(&Turn::edit_then_error("m.ts", c, ERR)));
    }
    assert_eq!(last.unwrap().severity, Severity::Trip);
}

#[test]
fn suggests_a_read_but_unedited_file() {
    let mut det = ThrashDetector::with_default_policy();
    // The agent keeps reading session.ts but only ever edits auth.ts. A recurs
    // to the trip threshold (turns 0, 2, 4).
    let seq = [
        edit("auth.ts", A, &["session.ts"], Observation::error(ERR)),
        edit("auth.ts", B, &["session.ts"], Observation::error(ERR)),
        edit("auth.ts", A, &[], Observation::error(ERR)),
        edit("auth.ts", B, &[], Observation::error(ERR)),
        edit("auth.ts", A, &[], Observation::error(ERR)),
    ];
    let mut interv = None;
    for t in seq {
        let v = det.ingest(&t);
        if let Some(i) = v.intervention {
            interv = Some(i);
        }
    }
    let interv = interv.expect("should trip");
    assert_eq!(interv.suggested_file.as_deref(), Some("session.ts"));
    assert!(interv.system_message.contains("session.ts"));
}

#[test]
fn aggressive_policy_trips_faster() {
    let mut det = ThrashDetector::new(ThrashPolicy::aggressive()); // trip_at = 2
    let v1 = det.ingest(&Turn::edit_then_error("a.ts", A, ERR));
    let v2 = det.ingest(&Turn::edit_then_error("a.ts", A, ERR));
    assert_eq!(v1.severity, Severity::Warn);
    assert_eq!(v2.severity, Severity::Trip);
}
