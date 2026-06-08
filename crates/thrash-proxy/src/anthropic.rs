//! Adapter between the Anthropic Messages API wire format and the
//! transport-agnostic [`Turn`] model that `thrash-core` understands.
//!
//! This is necessarily heuristic: we recover the agent's *behaviour* (which
//! files it wrote, which it read, what outcome it then saw) from the tool-use
//! and tool-result blocks in the `messages` array. The mapping rules are
//! documented inline and intentionally conservative — when in doubt we treat a
//! block as neutral so the breaker never trips on noise.

use serde_json::{json, Value};
use thrash_core::{
    FileEdit, Intervention, Observation, ObservationKind, ThrashDetector, ThrashPolicy, Turn,
};

/// The beta header that enables injecting `role: "system"` messages mid-session
/// without invalidating the prompt cache.
pub const MID_CONVERSATION_BETA: &str = "mid-conversation-system-2026-04-07";

/// Analyse a Messages API request body. Returns an [`Intervention`] iff the most
/// recent turn in the conversation crosses the trip threshold — i.e. the agent
/// is about to take another step from inside a confirmed loop.
pub fn analyze(body: &Value, policy: ThrashPolicy) -> Option<Intervention> {
    let turns = extract_turns(body);
    if turns.is_empty() {
        return None;
    }
    let mut det = ThrashDetector::new(policy);
    let mut last = None;
    for t in &turns {
        last = Some(det.ingest(t));
    }
    last.and_then(|v| v.intervention)
}

/// Inject the intervention as a trailing `role: "system"` message and return the
/// modified body. Safe to call only when `analyze` returned `Some`.
pub fn inject(mut body: Value, interv: &Intervention) -> Value {
    if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) {
        messages.push(json!({
            "role": "system",
            "content": interv.system_message,
        }));
    }
    body
}

/// Recover the ordered list of behavioural turns from a request body.
///
/// Grouping rule: accumulate edits/reads from assistant `tool_use` blocks, and
/// close a [`Turn`] when a `tool_result` reports a *real outcome* (an error or a
/// success). Neutral acknowledgements ("file written") do not close a turn, so
/// an edit stays attached to the test run that actually judges it.
pub fn extract_turns(body: &Value) -> Vec<Turn> {
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut turns = Vec::new();
    let mut pending_edits: Vec<FileEdit> = Vec::new();
    let mut pending_reads: Vec<String> = Vec::new();

    for msg in messages {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("");
        let blocks = content_blocks(msg);

        match role {
            "assistant" => {
                for b in &blocks {
                    classify_tool_use(b, &mut pending_edits, &mut pending_reads);
                }
            }
            "user" => {
                if let Some(obs) = strongest_observation(&blocks) {
                    if matches!(obs.kind, ObservationKind::Error | ObservationKind::Success) {
                        turns.push(Turn {
                            edits: std::mem::take(&mut pending_edits),
                            reads: std::mem::take(&mut pending_reads),
                            observation: obs,
                            note: None,
                        });
                    }
                    // Neutral results don't flush — keep accumulating.
                }
            }
            _ => {}
        }
    }

    turns
}

/// Normalise a message's `content` to a slice of block objects. A bare string
/// content yields an empty slice (no tool activity to mine).
fn content_blocks(msg: &Value) -> Vec<Value> {
    match msg.get("content") {
        Some(Value::Array(arr)) => arr.clone(),
        _ => Vec::new(),
    }
}

/// Inspect one assistant block; record an edit or a read if it is a file tool.
fn classify_tool_use(block: &Value, edits: &mut Vec<FileEdit>, reads: &mut Vec<String>) {
    if block.get("type").and_then(Value::as_str) != Some("tool_use") {
        return;
    }
    let name = block.get("name").and_then(Value::as_str).unwrap_or("");
    let input = block.get("input").cloned().unwrap_or(Value::Null);
    let command = input.get("command").and_then(Value::as_str);

    let path = input
        .get("path")
        .or_else(|| input.get("file_path"))
        .and_then(Value::as_str);

    let is_read = name_matches(name, &["read", "view", "open", "cat"]) || command == Some("view");
    let is_edit = name_matches(name, &["edit", "write", "str_replace", "create", "insert"])
        || matches!(command, Some("str_replace" | "create" | "insert" | "write"));

    if is_edit {
        if let Some(p) = path {
            let content = input
                .get("file_text")
                .or_else(|| input.get("content"))
                .or_else(|| input.get("new_str"))
                .or_else(|| input.get("new_string"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            edits.push(FileEdit {
                path: p.to_string(),
                content,
            });
        }
    } else if is_read {
        if let Some(p) = path {
            reads.push(p.to_string());
        }
    }
}

/// Reduce a user message's `tool_result` blocks to a single observation —
/// Error dominates Success dominates Neutral.
fn strongest_observation(blocks: &[Value]) -> Option<Observation> {
    let mut best: Option<Observation> = None;
    for b in blocks {
        if b.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        let is_error_flag = b.get("is_error").and_then(Value::as_bool).unwrap_or(false);
        let text = tool_result_text(b);
        let obs = if is_error_flag {
            Observation::error(signature_of(&text))
        } else {
            classify_text(&text)
        };
        best = Some(match best {
            Some(prev) => stronger(prev, obs),
            None => obs,
        });
    }
    best
}

fn stronger(a: Observation, b: Observation) -> Observation {
    let rank = |k: ObservationKind| match k {
        ObservationKind::Error => 2,
        ObservationKind::Success => 1,
        ObservationKind::Neutral => 0,
    };
    if rank(b.kind) >= rank(a.kind) {
        b
    } else {
        a
    }
}

/// Extract the text payload of a `tool_result` block (string or block array).
fn tool_result_text(block: &Value) -> String {
    match block.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Classify free-text command/test output without an explicit error flag.
fn classify_text(text: &str) -> Observation {
    let lower = text.to_lowercase();
    const ERROR_MARKERS: &[&str] = &[
        "error",
        "failed",
        "failure",
        " fail",
        "traceback",
        "panic",
        "exception",
        "assertionerror",
        "not ok",
        "✗",
        "fatal",
    ];
    const SUCCESS_MARKERS: &[&str] = &[
        "passed",
        "all tests pass",
        " ok",
        "success",
        "✓",
        "0 failures",
        "build succeeded",
    ];

    if ERROR_MARKERS.iter().any(|m| lower.contains(m)) {
        Observation::error(signature_of(text))
    } else if SUCCESS_MARKERS.iter().any(|m| lower.contains(m)) {
        Observation::success()
    } else {
        Observation::neutral(signature_of(text))
    }
}

/// Pick a stable signature line from multi-line output: the first line that
/// looks like an error, else the first non-empty line, truncated.
fn signature_of(text: &str) -> String {
    let pick = text
        .lines()
        .find(|l| {
            let l = l.to_lowercase();
            l.contains("error")
                || l.contains("fail")
                || l.contains("exception")
                || l.contains("panic")
        })
        .or_else(|| text.lines().find(|l| !l.trim().is_empty()))
        .unwrap_or("")
        .trim();
    pick.chars().take(200).collect()
}

fn name_matches(name: &str, needles: &[&str]) -> bool {
    let n = name.to_lowercase();
    needles.iter().any(|needle| n.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(messages: Value) -> Value {
        json!({ "model": "claude-opus-4-8", "messages": messages })
    }

    fn edit_msg(path: &str, content: &str) -> Value {
        json!({
            "role": "assistant",
            "content": [{
                "type": "tool_use", "id": "t1", "name": "str_replace_based_edit_tool",
                "input": { "command": "create", "path": path, "file_text": content }
            }]
        })
    }

    fn test_result(text: &str, is_error: bool) -> Value {
        json!({
            "role": "user",
            "content": [{ "type": "tool_result", "tool_use_id": "t2", "content": text, "is_error": is_error }]
        })
    }

    #[test]
    fn extracts_edit_and_error_as_one_turn() {
        let body = req(json!([
            edit_msg("auth.ts", "return s.token"),
            json!({"role":"assistant","content":[{"type":"tool_use","id":"t2","name":"bash","input":{"command_line":"npm test"}}]}),
            test_result("FAIL TypeError: cannot read 'token' of undefined", true),
        ]));
        let turns = extract_turns(&body);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].edits.len(), 1);
        assert_eq!(turns[0].edits[0].path, "auth.ts");
        assert_eq!(turns[0].observation.kind, ObservationKind::Error);
    }

    #[test]
    fn end_to_end_trip_and_inject() {
        let a = "return s.token";
        let b = "return s && s.token";
        let err = "FAIL TypeError: cannot read 'token' of undefined";
        let mut messages = Vec::new();
        for c in [a, b, a, b, a] {
            messages.push(edit_msg("auth.ts", c));
            messages.push(test_result(err, true));
        }
        let body = req(Value::Array(messages));

        let interv = analyze(&body, ThrashPolicy::default()).expect("should trip");
        assert_eq!(interv.thrashed_file, "auth.ts");

        let injected = inject(body, &interv);
        let msgs = injected["messages"].as_array().unwrap();
        let last = msgs.last().unwrap();
        assert_eq!(last["role"], "system");
        assert!(last["content"].as_str().unwrap().contains("auth.ts"));
    }

    #[test]
    fn healthy_session_does_not_inject() {
        let body = req(json!([
            edit_msg("config.ts", "v1"),
            test_result("FAIL E1", true),
            edit_msg("config.ts", "v2 improved"),
            test_result("All tests passed", false),
        ]));
        assert!(analyze(&body, ThrashPolicy::default()).is_none());
    }
}
