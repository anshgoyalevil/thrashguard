# ThrashGuard — Build Plan

A rigid, phased plan to take the agent "thrashing circuit breaker" from POC to
product. Each phase has explicit deliverables and exit criteria. Phase 0 and the
core of Phase 1 are **implemented in this repository today**.

---

## Problem statement

AI coding agents fall into *hallucination loops*: edit a file → run a test → see
an error → revert → run the test → see the same error → repeat, until the token
budget is drained. Existing guardrails watch token *volume*; none watch the
*behavioural semantics* of the loop. ThrashGuard models the agent's actions as a
state machine, detects cyclic `State A → State B → State A` patterns, and injects
a trusted operator note to break the loop — using the Anthropic
`mid-conversation-system` channel so the injection does not invalidate the prompt
cache.

**Success metric:** detect a real thrash loop within N≤1 cycle of crossing the
threshold, inject a correctly-targeted intervention, and produce zero false
positives on healthy forward-progress sessions.

---

## Architecture at a glance

```
agent CLI ──HTTPS──▶ thrash-proxy ──HTTPS──▶ api.anthropic.com
                         │
                         ├─ extract Turns from the Messages API body
                         ├─ thrash-core: ingest → Verdict (ok | warn | trip)
                         └─ on trip: append role:"system" note + beta header
```

Three crates:

| Crate          | Role                                                              | Status |
| -------------- | ---------------------------------------------------------------- | ------ |
| `thrash-core`  | Transport-agnostic detection engine (the IP). Fully unit-tested. | ✅ done |
| `thrash-demo`  | Replay-based terminal visualiser for live demos.                 | ✅ done |
| `thrash-proxy` | Transparent Anthropic Messages API proxy with injection.         | ✅ MVP  |

---

## Phase 0 — Foundations & POC  ✅ (in this repo)

**Goal:** a runnable, demonstrable detection engine plus a watchable demo.

Deliverables:
- [x] Cargo workspace, MIT license, CI, `.gitignore`, release profile tuned for the hot path.
- [x] `thrash-core`: `Turn`/`Observation` model, whitespace-insensitive fingerprinting, Levenshtein near-duplicate folding, streaming `ThrashDetector`, graduated `ok/warn/trip` verdicts, intervention builder.
- [x] Unit + integration tests (15) covering: classic A→B→A trip, trip-once-per-state, healthy progress (no false positive), passing-state-is-not-a-loop, near-duplicate folding, redirect-file suggestion, aggressive policy.
- [x] `thrash-demo` with two fixtures (`thrash_auth`, `healthy_progress`) and a boxed, colour-coded turn-by-turn visualisation.

**Exit criteria (met):** `cargo test` green; `cargo run -p thrash-demo` shows a trip on the loop fixture and silence on the control fixture.

---

## Phase 1 — Real proxy MVP  ✅ (in this repo)

**Goal:** sit transparently in front of the Anthropic API and inject on a real
request body.

Deliverables:
- [x] `axum` reverse proxy: `/v1/messages` interception + catch-all passthrough + `/healthz`.
- [x] Messages API → `Turn` extractor (tool_use edits/reads, tool_result outcomes, neutral-ack grouping).
- [x] Injection of a trailing `role:"system"` message + `anthropic-beta: mid-conversation-system-2026-04-07` header merge.
- [x] Env-configurable thresholds and upstream; structured `tracing` logs.
- [x] End-to-end proxy unit tests (extract → analyze → inject).

**Exit criteria (met):** proxy boots, `/healthz` responds, unit tests prove a
looped conversation gets an injected system note and a healthy one does not.

### Phase 1.1 — Hardening (next)
- [ ] SSE **streaming pass-through** (today responses are buffered). Stream upstream bytes back unmodified; only the request is rewritten.
- [ ] Per-conversation state keyed by a stable id (avoid full-history replay each request); persistent re-nudge policy across requests.
- [ ] Robust tool-name mapping table for common agent CLIs (Claude Code, Aider, Cline) + a recorded-traffic corpus for regression.
- [ ] TLS termination / cert guidance for CLIs that pin, or a `ANTHROPIC_BASE_URL` integration doc per CLI.

---

## Phase 2 — Detection quality

**Goal:** raise recall/precision on messy real traffic.

- [ ] **Semantic outcome clustering:** normalise error signatures (strip line numbers, addresses, timestamps) so "the same error" is robust to noise.
- [ ] **Multi-file cycles:** detect loops that span 2–3 files (A.ts↔B.ts ping-pong), not just single-file oscillation.
- [ ] **Edit-distance trajectories:** flag "shrinking progress" (each cycle changes less and less) as an early warning before an exact repeat.
- [ ] **Configurable redirect heuristics:** rank suggestion targets by import graph / error-trace mentions, not just read frequency.
- [ ] Labelled evaluation set + precision/recall dashboard; tune default thresholds against it.

**Exit criteria:** ≥0.9 precision and ≥0.8 recall on the labelled corpus; documented false-positive rate on healthy sessions.

---

## Phase 3 — Productisation

- [ ] Telemetry: per-session loop events, tokens-saved estimate (counterfactual: cycles-prevented × avg cycle cost).
- [ ] Policy config file (`thrashguard.toml`): thresholds, per-repo allow/deny, intervention templates.
- [ ] Pluggable interventions: system-note (default), hard-stop, human-in-the-loop webhook.
- [ ] Provider adapters behind a trait (`WireAdapter`): Anthropic first; OpenAI/others later.
- [ ] Distribution: single static binary, Homebrew tap, Docker image.

---

## Risks & mitigations

| Risk | Mitigation |
| ---- | ---------- |
| False positives interrupt legitimate retries | Graduated warn→trip; trip-once-per-state; conservative defaults; healthy-session test gate. |
| Frontier models already self-correct (task budgets, Opus 4.8) | Position as a budget/safety governor across the long tail of models & harnesses; measure tokens-saved. |
| CLI TLS pinning blocks the proxy | Document `ANTHROPIC_BASE_URL` redirection per CLI; offer a library-embed mode. |
| Wire-format drift | Adapter trait + recorded-traffic regression corpus. |

---

## How to run (today)

```bash
cargo test                                            # 15 tests, all green
cargo run -p thrash-demo                               # built-in auth.ts loop
cargo run -p thrash-demo -- fixtures/healthy_progress.json   # control (no trip)
THRASH_ADDR=127.0.0.1:8787 cargo run -p thrash-proxy   # live proxy
```
