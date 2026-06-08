//! ThrashGuard transparent proxy.
//!
//! Point an agent CLI's Anthropic base URL at this proxy. It forwards every
//! request upstream unchanged — except that it inspects each `/v1/messages`
//! body for a confirmed thrash loop and, when it finds one, appends a
//! `role: "system"` operator note (the circuit-breaker intervention) before
//! forwarding, adding the `mid-conversation-system` beta header so the
//! injection doesn't break the prompt cache.
//!
//! Configuration (env):
//!   THRASH_ADDR       listen address           (default 127.0.0.1:8787)
//!   THRASH_UPSTREAM   upstream API base URL    (default https://api.anthropic.com)
//!   THRASH_TRIP_AT    trip threshold           (default 3)
//!   THRASH_WARN_AT    warn threshold           (default 2)
//!
//! NOTE: v1 buffers responses (no SSE streaming pass-through yet) — see
//! docs/architecture.md. Fine for the demo and for non-streaming clients.

mod anthropic;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get, post},
    Router,
};
use serde_json::Value;
use thrash_core::ThrashPolicy;
use tracing::{info, warn};

#[derive(Clone)]
struct AppState {
    upstream: String,
    policy: ThrashPolicy,
    http: reqwest::Client,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "thrash_proxy=info".into()),
        )
        .init();

    let addr: SocketAddr = std::env::var("THRASH_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8787".into())
        .parse()
        .expect("invalid THRASH_ADDR");
    let upstream =
        std::env::var("THRASH_UPSTREAM").unwrap_or_else(|_| "https://api.anthropic.com".into());

    let policy = ThrashPolicy {
        trip_at: env_u32("THRASH_TRIP_AT", 3),
        warn_at: env_u32("THRASH_WARN_AT", 2),
        ..ThrashPolicy::default()
    };

    let state = AppState {
        upstream: upstream.clone(),
        policy,
        http: reqwest::Client::new(),
    };

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/v1/messages", post(handle_messages))
        .fallback(any(passthrough))
        .with_state(Arc::new(state));

    info!(%addr, %upstream, trip_at = policy.trip_at, "ThrashGuard proxy listening");

    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}

/// Intercept `/v1/messages`: analyse the conversation, inject on a confirmed
/// loop, then forward.
async fn handle_messages(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut beta_to_add: Option<&str> = None;

    let forward_body: Bytes = match serde_json::from_slice::<Value>(&body) {
        Ok(json) => match anthropic::analyze(&json, state.policy) {
            Some(interv) => {
                warn!(
                    file = %interv.thrashed_file,
                    occurrences = interv.occurrences,
                    suggested = ?interv.suggested_file,
                    "thrash loop detected — injecting circuit-breaker note"
                );
                beta_to_add = Some(anthropic::MID_CONVERSATION_BETA);
                let injected = anthropic::inject(json, &interv);
                match serde_json::to_vec(&injected) {
                    Ok(v) => Bytes::from(v),
                    Err(_) => body.clone(),
                }
            }
            None => body.clone(),
        },
        Err(_) => body.clone(), // not JSON we understand; pass through verbatim
    };

    forward(&state, "/v1/messages", &headers, forward_body, beta_to_add).await
}

/// Forward any other endpoint untouched (count_tokens, models, batches, …).
async fn passthrough(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
    body: Bytes,
) -> Response {
    let path = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    forward(&state, path, &headers, body, None).await
}

/// Shared upstream forwarder.
async fn forward(
    state: &AppState,
    path: &str,
    headers: &HeaderMap,
    body: Bytes,
    add_beta: Option<&str>,
) -> Response {
    let url = format!("{}{}", state.upstream.trim_end_matches('/'), path);
    let out_headers = build_forward_headers(headers, add_beta);

    let resp = state
        .http
        .post(&url)
        .headers(out_headers)
        .body(body)
        .send()
        .await;

    match resp {
        Ok(r) => {
            let status =
                StatusCode::from_u16(r.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let ct = r
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/json")
                .to_string();
            let bytes = r.bytes().await.unwrap_or_default();
            (status, [(axum::http::header::CONTENT_TYPE, ct)], bytes).into_response()
        }
        Err(e) => {
            warn!(error = %e, "upstream request failed");
            (
                StatusCode::BAD_GATEWAY,
                format!("thrash-proxy: upstream error: {e}"),
            )
                .into_response()
        }
    }
}

/// Copy through the auth/version headers an Anthropic request needs, merging in
/// the mid-conversation beta when we injected. Hop-by-hop and host headers are
/// dropped so reqwest sets them correctly.
fn build_forward_headers(
    incoming: &HeaderMap,
    add_beta: Option<&str>,
) -> reqwest::header::HeaderMap {
    let mut out = reqwest::header::HeaderMap::new();
    let mut beta_values: Vec<String> = Vec::new();

    for (name, value) in incoming.iter() {
        let n = name.as_str().to_ascii_lowercase();
        match n.as_str() {
            "host" | "content-length" | "connection" | "accept-encoding" => continue,
            "anthropic-beta" => {
                if let Ok(s) = value.to_str() {
                    beta_values.extend(s.split(',').map(|p| p.trim().to_string()));
                }
            }
            _ => {
                if let (Ok(hn), Ok(hv)) = (
                    HeaderName::from_bytes(name.as_ref()),
                    HeaderValue::from_bytes(value.as_bytes()),
                ) {
                    // reqwest uses the same http crate types; convert by bytes.
                    if let (Ok(rn), Ok(rv)) = (
                        reqwest::header::HeaderName::from_bytes(hn.as_ref()),
                        reqwest::header::HeaderValue::from_bytes(hv.as_bytes()),
                    ) {
                        out.insert(rn, rv);
                    }
                }
            }
        }
    }

    if let Some(b) = add_beta {
        if !beta_values.iter().any(|v| v == b) {
            beta_values.push(b.to_string());
        }
    }
    if !beta_values.is_empty() {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(&beta_values.join(", ")) {
            out.insert("anthropic-beta", v);
        }
    }

    out
}

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}
