//! Server-side proxy for AVNU's SNIP-29 paymaster.
//!
//! The desktop/mobile clients never hold `AVNU_API_KEY`. They call this proxy;
//! the proxy injects `x-paymaster-api-key` from `process`-equivalent env and
//! forwards JSON-RPC to AVNU.
//!
//! # Security
//!
//! Default (no `PROXY_PUBLIC`):
//! - Bind is loopback only (`127.0.0.1:8788`).
//! - Non-loopback binds require `PROXY_AUTH_TOKEN` (Bearer) on every `/` and
//!   `/rpc` request.
//! - No `CORS Any` — the product path is Tauri `reqwest`, not a browser.
//!
//! Product open mode (`PROXY_PUBLIC=1`):
//! - Non-loopback bind is allowed without `PROXY_AUTH_TOKEN`.
//! - `/` and `/rpc` do not require Bearer.
//! - Abuse control is AVNU credits / upstream — not a shared secret.
//! - Desktop product URL (`https://paymaster.bitcoinmarkets.app`) needs no
//!   client args or process-env token; Bearer is only for custom proxy URLs.
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
//!                       Non-loopback requires PROXY_AUTH_TOKEN unless
//!                       PROXY_PUBLIC=1.
//! PROXY_AUTH_TOKEN      Shared secret; required when binding off-loopback
//!                       without PROXY_PUBLIC. Clients send
//!                       `Authorization: Bearer <token>`.
//! PROXY_PUBLIC          Set to `1` for the hosted product paymaster: allow
//!                       non-loopback without Bearer. Default unset keeps
//!                       local/dev fail-closed.
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
         unauthenticated open relay, or set PROXY_PUBLIC=1 for the product \
         paymaster (abuse control = AVNU credits)"
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

/// Product-open bind: non-loopback without Bearer. Env truthy values: `1`/`true`/`yes`.
fn proxy_public_enabled(raw: Option<&str>) -> bool {
    matches!(
        raw.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

/// Resolve whether Bearer auth is required for this bind.
///
/// Returns `Ok(Some(token))` when auth is on, `Ok(None)` for loopback or
/// `PROXY_PUBLIC`, and `Err` for non-loopback without token or public flag.
fn resolve_auth_token(
    addr: &SocketAddr,
    bind_raw: &str,
    public: bool,
    token_env: Option<&str>,
) -> Result<Option<String>, BootError> {
    let token = token_env
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    if public {
        return Ok(None);
    }
    if !is_loopback(addr) && token.is_none() {
        return Err(BootError::NonLoopbackRequiresAuth(bind_raw.to_string()));
    }
    Ok(token)
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

    let public = proxy_public_enabled(std::env::var("PROXY_PUBLIC").ok().as_deref());
    let auth_token = resolve_auth_token(
        &addr,
        &bind_raw,
        public,
        std::env::var("PROXY_AUTH_TOKEN").ok().as_deref(),
    )?;

    if public {
        warn!(
            %addr,
            "PROXY_PUBLIC=1: open product paymaster — / and /rpc require no Bearer; \
             abuse control is AVNU credits / upstream"
        );
    } else if auth_token.is_some() {
        info!(%addr, "buzz-avnu-proxy requiring Bearer on / and /rpc");
    } else {
        info!(%addr, "buzz-avnu-proxy loopback-only (no Bearer)");
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
        // Loopback bind without token, or PROXY_PUBLIC product mode.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_public_env_truthy() {
        assert!(proxy_public_enabled(Some("1")));
        assert!(proxy_public_enabled(Some("true")));
        assert!(proxy_public_enabled(Some("YES")));
        assert!(!proxy_public_enabled(None));
        assert!(!proxy_public_enabled(Some("")));
        assert!(!proxy_public_enabled(Some("0")));
        assert!(!proxy_public_enabled(Some("false")));
    }

    #[test]
    fn non_loopback_without_token_fails_closed() {
        let addr: SocketAddr = "0.0.0.0:8788".parse().unwrap();
        let err = resolve_auth_token(&addr, "0.0.0.0:8788", false, None)
            .expect_err("must require token or PROXY_PUBLIC");
        assert!(matches!(err, BootError::NonLoopbackRequiresAuth(_)));
    }

    #[test]
    fn proxy_public_allows_non_loopback_without_token() {
        let addr: SocketAddr = "0.0.0.0:8788".parse().unwrap();
        assert_eq!(
            resolve_auth_token(&addr, "0.0.0.0:8788", true, None).expect("public"),
            None
        );
        // Public mode ignores a present token — product path is open.
        assert_eq!(
            resolve_auth_token(&addr, "0.0.0.0:8788", true, Some("unused")).expect("public"),
            None
        );
    }

    #[test]
    fn loopback_allows_missing_token() {
        let addr: SocketAddr = "127.0.0.1:8788".parse().unwrap();
        assert_eq!(
            resolve_auth_token(&addr, "127.0.0.1:8788", false, None).expect("loopback"),
            None
        );
    }

    #[test]
    fn non_loopback_with_token_keeps_bearer() {
        let addr: SocketAddr = "0.0.0.0:8788".parse().unwrap();
        assert_eq!(
            resolve_auth_token(&addr, "0.0.0.0:8788", false, Some("secret")).expect("auth"),
            Some("secret".into())
        );
    }
}
