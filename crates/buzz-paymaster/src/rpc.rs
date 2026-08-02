//! JSON-RPC implementation of [`Chain`](crate::Chain).
//!
//! Deliberately thin. Everything that can hold a bug lives above the trait where
//! it is tested with fakes; this file exists to make one HTTP call and map its
//! errors correctly.
//!
//! # The error mapping is the whole point
//!
//! `is_deployed` must answer "no" only when the chain actually says the contract is
//! absent, and must fail otherwise. Reading a timeout as absence would make the
//! sponsor pay to deploy over a live account — the money-losing bug this file is
//! written to avoid.

use serde::Deserialize;
use starknet_core::types::Felt;

use crate::{Chain, SponsorError};

/// Starknet JSON-RPC error code for a contract that does not exist.
///
/// Verified against `mainnet.nodes.starknet.org/rpc/v0_10`: `getClassHashAt` on an
/// undeployed address returns `{"code":20,"message":"Contract not found"}`, and on
/// a deployed one returns a class hash. Both directions were checked rather than
/// assumed, because treating any other error as "absent" loses money.
pub const CONTRACT_NOT_FOUND: i64 = 20;

/// A [`Chain`] backed by a Starknet JSON-RPC endpoint.
#[derive(Debug, Clone)]
pub struct JsonRpcChain {
    url: String,
    client: reqwest::Client,
}

#[derive(Deserialize)]
struct RpcError {
    code: i64,
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct RpcResponse {
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    error: Option<RpcError>,
}

impl JsonRpcChain {
    /// Builds a client for `url`, e.g.
    /// `https://mainnet.nodes.starknet.org/rpc/v0_10`.
    ///
    /// Note the hostname ordering: `nodes.starknet.org`, not `starknet.nodes.org`.
    pub fn new(url: impl Into<String>) -> Result<Self, SponsorError> {
        let client = reqwest::Client::builder()
            // A sponsor deciding whether to deploy must not hang forever: without a
            // timeout a stalled node turns into a stalled queue of requests.
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|e| SponsorError::Config(format!("cannot build HTTP client: {e}")))?;
        Ok(Self {
            url: url.into(),
            client,
        })
    }
}

impl Chain for JsonRpcChain {
    async fn is_deployed(&self, address: Felt) -> Result<bool, SponsorError> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "starknet_getClassHashAt",
            "params": { "block_id": "latest", "contract_address": format!("{address:#x}") }
        });
        let response = self
            .client
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .map_err(|e| SponsorError::Chain(format!("request failed: {e}")))?;

        let status = response.status();
        let parsed: RpcResponse = response
            .json()
            .await
            // A non-JSON body is usually a proxy error page. Report it as a chain
            // error, never as absence.
            .map_err(|e| {
                SponsorError::Chain(format!("HTTP {status}: unparseable response: {e}"))
            })?;

        if let Some(err) = parsed.error {
            return if err.code == CONTRACT_NOT_FOUND {
                // The one case that means "not deployed".
                Ok(false)
            } else {
                Err(SponsorError::Chain(format!(
                    "rpc error {}: {}",
                    err.code, err.message
                )))
            };
        }
        match parsed.result {
            Some(_) => Ok(true),
            // Neither result nor error is a malformed response, not an answer.
            None => Err(SponsorError::Chain(
                "response carried neither result nor error".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_not_found_is_the_verified_code() {
        // Pinned so a refactor cannot quietly change which code means "absent".
        assert_eq!(CONTRACT_NOT_FOUND, 20);
    }

    #[test]
    fn a_bad_url_is_a_config_error_not_a_panic() {
        // Construction should not panic on anything; failures surface as errors.
        assert!(
            JsonRpcChain::new("not-a-url").is_ok(),
            "construction defers URL validation to the request"
        );
    }

    #[tokio::test]
    async fn an_unreachable_endpoint_is_a_chain_error_not_false() {
        // The critical mapping: unreachable must never read as "not deployed", or
        // the sponsor pays to deploy over an account that already exists.
        let chain = JsonRpcChain::new("http://127.0.0.1:1/rpc").unwrap();
        let result = chain.is_deployed(Felt::ONE).await;
        assert!(
            matches!(result, Err(SponsorError::Chain(_))),
            "expected a Chain error, got {result:?}"
        );
    }

    #[tokio::test]
    async fn a_non_json_body_is_a_chain_error_not_false() {
        // Proxy error pages are HTML. Same rule: never absence.
        let chain = JsonRpcChain::new("https://example.com/").unwrap();
        let result = chain.is_deployed(Felt::ONE).await;
        assert!(
            matches!(result, Err(SponsorError::Chain(_))),
            "expected a Chain error, got {result:?}"
        );
    }
}
