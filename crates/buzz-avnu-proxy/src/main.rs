//! Server-side proxy for AVNU's SNIP-29 paymaster.
//!
//! The desktop/mobile clients never hold `AVNU_API_KEY`. They call this proxy;
//! the proxy injects `x-paymaster-api-key` from `process`-equivalent env and
//! forwards JSON-RPC to AVNU.
//!
//! # Security
//!
//! This binary must **not** be an unauthenticated open relay:
//! - Default bind is loopback only (`127.0.0.1:8788`).
//! - Non-loopback binds require `PROXY_AUTH_TOKEN` (Bearer) on every `/` and
//!   `/rpc` request.
//! - No `CORS Any` — the product path is Tauri `reqwest`, not a browser.
//! - Production sponsorship is the AWS paymaster (egress-only, no ingress);
//!   do not expose this proxy on `0.0.0.0` without auth.
//!
//! # Required environment
//!
//! ```text
//! AVNU_API_KEY          Managed AVNU API key (never commit; never ship in the
//!                       Tauri binary or frontend bundle)
//! ```
//!
//! # Optional environment
//!
//! ```text
//! AVNU_PAYMASTER_URL    Upstream paymaster JSON-RPC endpoint.
//!                       Default: https://starknet.paymaster.avnu.fi
//!                       Test:    https://sepolia.paymaster.avnu.fi
//! BIND_ADDR             Listen address. Default: 127.0.0.1:8788 (loopback).
//!                       Non-loopback requires PROXY_AUTH_TOKEN.
//! PROXY_AUTH_TOKEN      Shared secret; required when binding off-loopback.
//!                       Clients send `Authorization: Bearer <token>`.
//!
//! INDEXER_URL           Required on listing clients (or product public host
//!                       https://markets.bitcoinmarkets.app). NO localhost
//!                       default — do not ship http://127.0.0.1:8787.
//!                       GET {INDEXER_URL}/api/markets, GET {INDEXER_URL}/health.
//!                       Never put ADMIN_API_KEY / AVNU_API_KEY in the repo.
//! ```

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use serde_json::Value;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tracing::{error, info, warn};

const DEFAULT_UPSTREAM: &str = "https://starknet.paymaster.avnu.fi";
/// Loopback-only default — never `0.0.0.0` (that would be an open relay).
const DEFAULT_BIND: &str = "127.0.0.1:8788";
const PRODUCT_INDEXER_URL: &str = "https://markets.bitcoinmarkets.app";

#[derive(Clone)]
struct AppState {
    client: reqwest::Client,
    upstream: String,
    api_key: String,
    /// When set, every JSON-RPC request must present this Bearer token.
    auth_token: Option<String>,
}

#[derive(Debug, thiserror::Error)]
enum BootError {
    #[error("AVNU_API_KEY is required (set it in the environment; never commit the value)")]
    MissingApiKey,
    #[error(
        "BIND_ADDR {0:?} is not loopback; set PROXY_AUTH_TOKEN so this is not an \
         unauthenticated open relay (production sponsorship is the AWS paymaster)"
    )]
    NonLoopbackRequiresAuth(String),
    #[error("invalid BIND_ADDR {0:?}: {1}")]
    BadBind(String, String),
    #[error("bind failed: {0}")]
    Bind(#[from] std::io::Error),
}

fn is_loopback(addr: &SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

#[tokio::main]
async fn main() -> Result<(), BootError> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let api_key = std::env::var("AVNU_API_KEY")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or(BootError::MissingApiKey)?;

    let upstream = std::env::var("AVNU_PAYMASTER_URL")
        .unwrap_or_else(|_| DEFAULT_UPSTREAM.to_string())
        .trim_end_matches('/')
        .to_string();

    // Proxy does not call the indexer. Listing clients use product host /
    // required INDEXER_URL — never invent a localhost default here.
    match std::env::var("INDEXER_URL") {
        Ok(url) if url.contains("127.0.0.1") || url.contains("localhost") => {
            warn!(
                indexer_url = %url,
                product = PRODUCT_INDEXER_URL,
                "INDEXER_URL points at loopback; use https://markets.bitcoinmarkets.app (no localhost default)"
            );
        }
        Ok(url) => info!(indexer_url = %url, "INDEXER_URL set"),
        Err(_) => info!(
            product = PRODUCT_INDEXER_URL,
            "INDEXER_URL unset on this process (clients use https://markets.bitcoinmarkets.app)"
        ),
    }

    let bind_raw = std::env::var("BIND_ADDR").unwrap_or_else(|_| DEFAULT_BIND.to_string());
    let addr: SocketAddr = bind_raw.parse().map_err(|e: std::net::AddrParseError| {
        BootError::BadBind(bind_raw.clone(), e.to_string())
    })?;

    let auth_token = std::env::var("PROXY_AUTH_TOKEN")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if !is_loopback(&addr) && auth_token.is_none() {
        return Err(BootError::NonLoopbackRequiresAuth(bind_raw));
    }

    let state = Arc::new(AppState {
        client: reqwest::Client::new(),
        upstream,
        api_key,
        auth_token,
    });

    // No CORS Any — this is not a browser-facing open relay.
    let app = Router::new()
        .route("/health", get(health))
        .route("/", post(proxy_rpc))
        .route("/rpc", post(proxy_rpc))
        .with_state(state);

    info!(%addr, "buzz-avnu-proxy listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> impl IntoResponse {
    (
        StatusCode::OK,
        axum::Json(serde_json::json!({
            "status": "ok",
            "service": "buzz-avnu-proxy",
        })),
    )
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        axum::Json(serde_json::json!({
            "jsonrpc": "2.0",
            "error": { "code": -32003, "message": "unauthorized" },
            "id": null
        })),
    )
        .into_response()
}

fn authorize(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(expected) = state.auth_token.as_deref() else {
        // Loopback bind without token — local-only.
        return true;
    };
    let Some(value) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let Some(token) = value.strip_prefix("Bearer ").map(str::trim) else {
        return false;
    };
    // Constant-time-ish compare for typical token lengths.
    token.len() == expected.len()
        && token
            .as_bytes()
            .iter()
            .zip(expected.as_bytes())
            .all(|(a, b)| a == b)
}

async fn proxy_rpc(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !authorize(&state, &headers) {
        return unauthorized();
    }

    // Validate JSON so we never forward garbage that could confuse operators'
    // logs into looking like a key leak.
    if let Err(e) = serde_json::from_slice::<Value>(&body) {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({
                "jsonrpc": "2.0",
                "error": { "code": -32700, "message": format!("parse error: {e}") },
                "id": null
            })),
        )
            .into_response();
    }

    let mut out_headers = HeaderMap::new();
    out_headers.insert("content-type", HeaderValue::from_static("application/json"));
    out_headers.insert("accept", HeaderValue::from_static("*/*"));
    // Inject the secret server-side. Never echo it back.
    match HeaderValue::from_str(&state.api_key) {
        Ok(v) => {
            out_headers.insert("x-paymaster-api-key", v);
        }
        Err(_) => {
            error!("AVNU_API_KEY contains characters illegal in an HTTP header");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "error": { "code": -32000, "message": "proxy misconfigured" },
                    "id": null
                })),
            )
                .into_response();
        }
    }

    let upstream = state
        .client
        .post(&state.upstream)
        .headers(out_headers)
        .body(body);

    match upstream.send().await {
        Ok(resp) => {
            let status =
                StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let bytes = resp.bytes().await.unwrap_or_default();
            (status, [("content-type", "application/json")], bytes).into_response()
        }
        Err(e) => {
            error!(error = %e, "upstream AVNU request failed");
            (
                StatusCode::BAD_GATEWAY,
                axum::Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "error": { "code": -32001, "message": "upstream unavailable" },
                    "id": null
                })),
            )
                .into_response()
        }
    }
}
