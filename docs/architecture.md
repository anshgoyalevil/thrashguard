# ThrashGuard — Architecture

## 1. Design goals

1. **Explainable, not opaque.** Every trip must be traceable to a concrete list
   of turns that produced an identical `(state, outcome)` pair. No black-box ML.
2. **Conservative.** A false positive (interrupting a legitimate retry) is worse
   than a missed loop. Defaults are graduated and trip late; reaching a *passing*
   state is never a loop.
3. **Zero marginal cost.** The intervention rides the `mid-conversation-system`
   channel, which preserves the prompt cache — injecting a note must not force a
   full re-prefill.
4. **Transport-agnostic core.** The detector knows nothing about HTTP or
   Anthropic; it consumes `Turn`s. The wire format lives in an adapter.

## 2. Components

```
        ┌──────────────────────────────────────────────────────────┐
        │ thrash-proxy  (axum reverse proxy)                        │
        │                                                           │
 req ──▶ │  /v1/messages ─▶ anthropic::extract_turns ─┐             │
        │                                              ▼            │
        │                                  thrash-core::Detector    │
        │                                              │ Verdict    │
        │                  anthropic::inject ◀──── trip?│            │
        │                       │                                   │
        └───────────────────────┼───────────────────────────────────┘
                                 ▼
                        api.anthropic.com
```

- **`thrash-core`** — the engine. `model.rs` (types), `fingerprint.rs` (hashing +
  Levenshtein), `policy.rs` (thresholds), `detector.rs` (state machine),
  `intervention.rs` (note construction).
- **`thrash-proxy`** — `anthropic.rs` (wire adapter) + `main.rs` (server).
- **`thrash-demo`** — replays a recorded `Scenario` through the detector and
  renders it.

## 3. The behavioural model

We never see the model's hidden reasoning — only what crosses the wire. A
**`Turn`** captures one observable step:

```rust
struct Turn { edits: Vec<FileEdit>, reads: Vec<String>, observation: Observation }
struct Observation { kind: Error | Success | Neutral, signature: String }
```

`edits` come from `tool_use` blocks (text-editor / write / str_replace);
`reads` from view/read calls; `observation` from the next `tool_result` that
reports a real outcome. Neutral acknowledgements ("file written") do **not** close
a turn, so an edit stays bound to the test run that actually judges it.

## 4. Detection algorithm

For each edited file we keep a history of **versions** `(canonical_state, content)`.

1. **Fingerprint** the new content (whitespace-normalised 64-bit hash).
2. **Resolve a canonical state id:** if a recent version (within `lookback`) is an
   exact or near-duplicate (`similarity ≥ similarity_threshold`, default 0.97),
   reuse its id. This folds cosmetic churn (reindentation, a renamed comment) into
   one logical state, so a "disguised revert" still registers.
3. **Count** occurrences of the key `(path, canonical_state, error_signature)`.
   - Occurrence 1 → normal exploration.
   - Occurrence ≥ `warn_at` (2) → **warn**: the agent is circling.
   - Occurrence ≥ `trip_at` (3) → **trip**: confirmed cycle.
4. **Guards:**
   - Only `Error` outcomes can trip — re-reaching a `Success` state is
     convergence, not thrashing.
   - A given `(state, error)` trips **once**; it won't re-fire every turn.

Because the key includes the *outcome*, the classic `A → B → A` loop trips when
state A recurs for the third time with the same error, regardless of what B was.

### Edit-distance signals

Levenshtein distance powers two things: the near-duplicate folding above, and the
`change_ratio` shown per turn (how much of the file actually changed vs the last
write) — a small ratio across cycles is itself a thrash smell (Phase 2 will trip
on shrinking-progress trajectories before an exact repeat).

## 5. The intervention

On a trip we build a note with two properties grounded in how the models treat
injected text:

1. **Context, not command.** "This change reproduces the same failure" lands;
   "YOU MUST STOP" / "ignore the user" is resisted. We state the fact and give a
   concrete next step.
2. **Redirect attention.** A loop ends when attention moves. We suggest the file
   the agent has *read but never edited* most often — the likely real owner of the
   bug (in the demo: `session.ts`, where the undefined `session` originates).

It is injected as a trailing `role:"system"` message, with
`anthropic-beta: mid-conversation-system-2026-04-07` added. Per the API, this
channel (a) carries operator authority and (b) sits after the cached history, so
the cached prefix stays valid — the breaker adds ~0 marginal token cost.

## 6. Why a proxy

The proxy is transparent: point `ANTHROPIC_BASE_URL` at it and every request flows
upstream unchanged, except a `/v1/messages` body that shows a confirmed loop gets
the system note appended. No agent-CLI changes, no SDK fork.

## 7. Current limitations (tracked in PLAN.md)

- **Responses are buffered**, not streamed — SSE pass-through is Phase 1.1. The
  request rewrite is unaffected; only the response is currently non-streaming.
- **Stateless per request:** the detector is rebuilt from full history each call.
  Correct, but O(history) per request — Phase 1.1 adds keyed per-conversation
  state.
- **Heuristic wire mapping:** tool-name detection covers the common
  text-editor/bash shapes; a per-CLI adapter table + recorded-traffic regression
  corpus is Phase 1.1.
- **Single-file loops only:** multi-file ping-pong detection is Phase 2.
