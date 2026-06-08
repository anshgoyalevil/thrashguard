# ThrashGuard ⚡

**A behavioural circuit breaker for AI coding agents.**

AI agents get stuck in *hallucination loops*: edit a file → run the test → see
the same error → revert → run the test → see the same error → repeat, burning the
token budget until it caps. ThrashGuard watches the agent's **behaviour** (not
just token volume), detects the cycle, and injects a trusted operator note to
break it — using Anthropic's `mid-conversation-system` channel so the
intervention **doesn't invalidate the prompt cache**.

```
   ok    turn 0  edit auth.ts        → ✗ TypeError: cannot read 'token' of undefined
   ok    turn 1  edit auth.ts  read session.ts
  warn   turn 2  edit auth.ts        → `auth.ts` returned to a prior failing state (occ #2)
  warn   turn 3  edit auth.ts
  TRIP   turn 4  edit auth.ts        → occurrence #3

  ╔══════════════════════════════════════════════════════════════╗
  ║ ⚡ CIRCUIT BREAKER TRIPPED — injecting operator note          ║
  ╟──────────────────────────────────────────────────────────────╢
  ║ System note: the edit just applied to `auth.ts` is the same  ║
  ║ change already attempted 3 times … inspect `session.ts` …    ║
  ╚══════════════════════════════════════════════════════════════╝
```

> Built in Rust for the request hot path: C-level throughput, memory safety, and
> the standard async stack (tokio/axum) used by production proxies.

---

## Quick start

```bash
# 1. Run the test suite (15 tests)
cargo test

# 2. Watch the demo — the canonical auth.ts loop, caught and broken
cargo run -p thrash-demo

# 3. Run the control scenario — healthy progress, breaker stays silent
cargo run -p thrash-demo -- fixtures/healthy_progress.json

# Demo flags:  --fast (no pauses)   --aggressive (trip on first repeat)
```

### Use the live proxy

Point your agent CLI's Anthropic base URL at the proxy; it forwards everything
upstream and only rewrites a request when it detects a loop.

```bash
THRASH_ADDR=127.0.0.1:8787 cargo run -p thrash-proxy

# then, for the agent CLI:
export ANTHROPIC_BASE_URL=http://127.0.0.1:8787
```

| Env var          | Default                    | Meaning                  |
| ---------------- | -------------------------- | ------------------------ |
| `THRASH_ADDR`    | `127.0.0.1:8787`           | listen address           |
| `THRASH_UPSTREAM`| `https://api.anthropic.com`| upstream API base URL    |
| `THRASH_TRIP_AT` | `3`                        | repeats before tripping  |
| `THRASH_WARN_AT` | `2`                        | repeats before warning   |

---

## How it works

1. **Observe.** From each `/v1/messages` body, recover the agent's *behaviour*:
   which files it wrote (`tool_use`), which it read, and the outcome it then saw
   (`tool_result`: error / success).
2. **Fingerprint.** Each file write gets a whitespace-insensitive fingerprint;
   near-identical versions (Levenshtein ≥ 97%) fold into one logical *state*, so
   cosmetic churn doesn't hide a revert.
3. **Detect.** Count occurrences of each `(file, state, error)` triple. The first
   is exploration; repeats are the agent re-entering a known-bad state. Warn at 2,
   **trip at 3** (configurable). Reaching a *passing* state never counts as a loop.
4. **Intervene.** On a trip, inject a `role:"system"` note that (a) states the
   loop as fact (not a command — the models resist override language) and
   (b) redirects attention to a file the agent *read but never fixed* (the likely
   real owner of the bug). The `mid-conversation-system` beta header keeps the
   cached prefix intact.

See [`docs/architecture.md`](docs/architecture.md) for the full design and
[`PLAN.md`](PLAN.md) for the phased roadmap.

---

## Project layout

```
thrashguard/
├── Cargo.toml                  # workspace
├── PLAN.md                     # phased build plan / roadmap
├── crates/
│   ├── thrash-core/            # the detection engine (transport-agnostic IP)
│   │   ├── src/{model,fingerprint,policy,detector,intervention}.rs
│   │   └── tests/detector_tests.rs
│   ├── thrash-demo/            # replay visualiser for live demos
│   └── thrash-proxy/           # transparent Anthropic Messages API proxy
├── fixtures/                   # replayable agent sessions (loop + control)
└── docs/architecture.md
```

---

## Status

POC complete: detection engine and demo are fully implemented and tested; the
proxy is a working MVP (request rewriting + injection). Streaming pass-through and
detection-quality work are tracked in [`PLAN.md`](PLAN.md).

## License

MIT — see [LICENSE](LICENSE).
