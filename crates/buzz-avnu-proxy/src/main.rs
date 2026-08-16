//! Server-side proxy for AVNU's SNIP-29 paymaster.
//!
//! The desktop/mobile clients never hold `AVNU_API_KEY`. They call this proxy;
//! the proxy injects `x-paymaster-api-key` from `process`-equivalent env and
//! forwards JSON-RPC to AVNU.
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
//! BIND_ADDR             Listen address. Default: 0.0.0.0:8788
//!
//! INDEXER_URL           Listing clients (desktop) default to
//!                       http://127.0.0.1:8787 (Adrien's machine). Override
//!                       for a public host. GET {INDEXER_URL}/api/markets,
//!                       GET {INDEXER_URL}/health. Cloud agents must not
//!                       live-fetch the default URL.
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
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info};

const DEFAULT_UPSTREAM: &str = "https://starknet.paymaster.avnu.fi";
const DEFAULT_BIND: &str = "0.0.0.0:8788";
const DEFAULT_INDEXER_URL: &str = "http://127.0.0.1:8787";

#[derive(Clone)]
struct AppState {
    client: reqwest::Client,
    upstream: String,
    api_key: String,
}

#[derive(Debug, thiserror::Error)]
enum BootError {
    #[error("AVNU_API_KEY is required (set it in the environment; never commit the value)")]
    MissingApiKey,
    #[error("invalid BIND_ADDR {0:?}: {1}")]
    BadBind(String, String),
    #[error("bind failed: {0}")]
    Bind(#[from] std::io::Error),
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

    // Proxy does not call the indexer. Listing clients default to Adrien's
    // localhost indexer; cloud agents must not live-fetch that host.
    match std::env::var("INDEXER_URL") {
        Ok(url) => info!(indexer_url = %url, "INDEXER_URL set"),
        Err(_) => info!(
            default = DEFAULT_INDEXER_URL,
            "INDEXER_URL unset on this process (desktop default is Adrien localhost indexer)"
        ),
    }

    let bind_raw = std::env::var("BIND_ADDR").unwrap_or_else(|_| DEFAULT_BIND.to_string());
    let addr: SocketAddr = bind_raw.parse().map_err(|e: std::net::AddrParseError| {
        BootError::BadBind(bind_raw.clone(), e.to_string())
    })?;

    let state = Arc::new(AppState {
        client: reqwest::Client::new(),
        upstream,
        api_key,
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(health))
        .route("/", post(proxy_rpc))
        .route("/rpc", post(proxy_rpc))
        .with_state(state)
        .layer(cors);

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

async fn proxy_rpc(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
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

    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        HeaderValue::from_static("application/json"),
    );
    headers.insert("accept", HeaderValue::from_static("*/*"));
    // Inject the secret server-side. Never echo it back.
    match HeaderValue::from_str(&state.api_key) {
        Ok(v) => {
            headers.insert("x-paymaster-api-key", v);
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

    let upstream = state.client.post(&state.upstream).headers(headers).body(body);

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
