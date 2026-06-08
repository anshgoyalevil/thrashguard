//! ThrashGuard demo: replay a recorded agent session through the detector and
//! visualise, turn by turn, how the circuit breaker builds confidence and
//! ultimately trips — printing the exact note it would inject to break the loop.
//!
//! Usage:
//!   thrash-demo [FIXTURE.json] [--fast] [--aggressive]
//!
//! With no fixture, a built-in "auth.ts thrash" scenario is used.

mod tui;

use std::time::Duration;

use thrash_core::{Scenario, Severity, ThrashDetector, ThrashPolicy, Turn, Verdict};
use tui::*;

const WIDTH: usize = 76;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let fast = args.iter().any(|a| a == "--fast");
    let aggressive = args.iter().any(|a| a == "--aggressive");
    let fixture_path = args.iter().find(|a| !a.starts_with("--"));

    let scenario = match fixture_path {
        Some(path) => match std::fs::read_to_string(path).map(|s| Scenario::from_json(&s)) {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                eprintln!("{} failed to parse {path}: {e}", paint(RED, "error:"));
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("{} cannot read {path}: {e}", paint(RED, "error:"));
                std::process::exit(1);
            }
        },
        None => builtin_scenario(),
    };

    let policy = if aggressive {
        ThrashPolicy::aggressive()
    } else {
        ThrashPolicy::default()
    };
    let delay = if fast {
        Duration::from_millis(0)
    } else {
        Duration::from_millis(850)
    };

    run(&scenario, policy, delay);
}

fn run(scenario: &Scenario, policy: ThrashPolicy, delay: Duration) {
    print_header(scenario, &policy);
    let mut det = ThrashDetector::new(policy);
    let mut tripped_once = false;

    for (i, turn) in scenario.turns.iter().enumerate() {
        let verdict = det.ingest(turn);
        print_turn(i, turn, &verdict);
        if let Some(interv) = &verdict.intervention {
            tripped_once = true;
            println!();
            println!(
                "{}",
                boxed(
                    "⚡ CIRCUIT BREAKER TRIPPED — injecting operator note",
                    RED,
                    &interv.system_message,
                    WIDTH,
                )
            );
            println!(
                "   {} {}",
                dim("wire effect:"),
                dim("prepend role=\"system\" message + header anthropic-beta: mid-conversation-system-2026-04-07")
            );
        }
        std::thread::sleep(delay);
    }

    println!();
    print_summary(tripped_once);
}

fn print_header(scenario: &Scenario, policy: &ThrashPolicy) {
    println!();
    println!(
        "{}",
        paint(
            CYAN,
            &bold("  ╔══ ThrashGuard ═══════════════════════════════════════════════════╗")
        )
    );
    println!(
        "{}",
        paint(
            CYAN,
            &bold("  ║  behavioural circuit breaker for AI coding agents                ║")
        )
    );
    println!(
        "{}",
        paint(
            CYAN,
            &bold("  ╚══════════════════════════════════════════════════════════════════╝")
        )
    );
    println!();
    println!("  {} {}", bold("scenario:"), scenario.scenario);
    if !scenario.description.is_empty() {
        println!("  {}", dim(&scenario.description));
    }
    println!(
        "  {} warn at {} repeats · trip at {} repeats · near-dup ≥ {:.0}%",
        bold("policy:"),
        policy.warn_at,
        policy.trip_at,
        policy.similarity_threshold * 100.0,
    );
    println!("  {}", dim(&"─".repeat(WIDTH)));
}

fn print_turn(i: usize, turn: &Turn, verdict: &Verdict) {
    let (badge, color) = match verdict.severity {
        Severity::Ok => ("  ok  ", GREEN),
        Severity::Warn => (" warn ", YELLOW),
        Severity::Trip => (" TRIP ", RED_BG),
    };

    let edits: Vec<String> = turn.edits.iter().map(|e| e.path.clone()).collect();
    let action = if edits.is_empty() {
        "(no edit)".to_string()
    } else {
        format!("edit {}", edits.join(", "))
    };
    let reads = if turn.reads.is_empty() {
        String::new()
    } else {
        format!("  {} {}", dim("read"), dim(&turn.reads.join(", ")))
    };

    let obs = match turn.observation.kind {
        thrash_core::ObservationKind::Error => {
            paint(RED, &format!("✗ {}", short(&turn.observation.signature)))
        }
        thrash_core::ObservationKind::Success => paint(GREEN, "✓ passed"),
        thrash_core::ObservationKind::Neutral => paint(GREY, &short(&turn.observation.signature)),
    };

    println!();
    println!(
        "  {} {}  {}{}",
        paint(color, &bold(badge)),
        bold(&format!("turn {i}")),
        action,
        reads,
    );
    println!("        {} {}", dim("→"), obs);

    for s in &verdict.signals {
        let line_color = match s.severity {
            Severity::Ok => GREY,
            Severity::Warn => YELLOW,
            Severity::Trip => RED,
        };
        println!(
            "        {} {}",
            paint(line_color, "•"),
            paint(line_color, &s.message)
        );
    }
}

fn print_summary(tripped: bool) {
    if tripped {
        println!(
            "  {}  Loop detected and broken before it could drain the token budget.",
            paint(GREEN, &bold("done.")),
        );
        println!(
            "  {}",
            dim("Without ThrashGuard the agent would keep re-applying the same fix until it hit its budget cap.")
        );
    } else {
        println!(
            "  {}  Healthy session — forward progress, breaker stayed quiet (no false positive).",
            paint(GREEN, &bold("done.")),
        );
    }
    println!();
}

fn short(s: &str) -> String {
    let first = s.lines().next().unwrap_or(s);
    if first.chars().count() > 60 {
        format!("{}…", first.chars().take(60).collect::<String>())
    } else {
        first.to_string()
    }
}

/// Built-in scenario used when no fixture is supplied: the canonical `auth.ts`
/// hallucination loop, where the real bug lives in `session.ts`.
fn builtin_scenario() -> Scenario {
    let a = "export function getToken(session) {\n  return session.token;\n}";
    let b = "export function getToken(session) {\n  return session && session.token;\n}";
    let err = "TypeError: cannot read property 'token' of undefined (auth.test.ts:12)";

    let mk = |content: &str, reads: &[&str], note: &str| Turn {
        edits: vec![thrash_core::FileEdit {
            path: "auth.ts".into(),
            content: content.into(),
        }],
        reads: reads.iter().map(|s| s.to_string()).collect(),
        observation: thrash_core::Observation::error(err),
        note: Some(note.into()),
    };

    Scenario {
        scenario: "auth.ts hallucination loop".into(),
        description:
            "The agent keeps rewriting getToken() in auth.ts, but the undefined session is created in session.ts."
                .into(),
        turns: vec![
            mk(a, &[], "first attempt"),
            mk(b, &["session.ts"], "add a guard"),
            mk(a, &["session.ts"], "revert the guard"),
            mk(b, &[], "re-add the guard"),
            mk(a, &[], "revert again"),
        ],
    }
}
