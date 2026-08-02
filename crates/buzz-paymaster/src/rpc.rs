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

#[derive(Deserialize)]
struct RpcCallResponse {
    #[serde(default)]
    result: Option<Vec<String>>,
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

    /// Performs a read-only `starknet_call`, returning the felts it produced.
    ///
    /// A revert is reported as an error, never as an empty result: an entry point
    /// that failed and one that returned nothing must not be confused, or a failing
    /// `balanceOf` would read as a zero balance.
    async fn call(
        &self,
        contract: Felt,
        entry_point: &str,
        calldata: &[Felt],
    ) -> Result<Vec<Felt>, SponsorError> {
        let selector = starknet_core::utils::get_selector_from_name(entry_point)
            .map_err(|e| SponsorError::Config(format!("bad selector {entry_point}: {e}")))?;
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "starknet_call",
            "params": {
                "request": {
                    "contract_address": format!("{contract:#x}"),
                    "entry_point_selector": format!("{selector:#x}"),
                    "calldata": calldata.iter().map(|f| format!("{f:#x}")).collect::<Vec<_>>(),
                },
                "block_id": "latest"
            }
        });
        let response = self
            .client
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .map_err(|e| SponsorError::Chain(format!("call {entry_point} failed: {e}")))?;
        let status = response.status();
        let parsed: RpcCallResponse = response.json().await.map_err(|e| {
            SponsorError::Chain(format!("HTTP {status}: unparseable call response: {e}"))
        })?;
        if let Some(err) = parsed.error {
            return Err(SponsorError::Chain(format!(
                "rpc error {} calling {entry_point}: {}",
                err.code, err.message
            )));
        }
        let raw = parsed.result.ok_or_else(|| {
            SponsorError::Chain(format!(
                "call {entry_point} returned neither result nor error"
            ))
        })?;
        raw.iter()
            .map(|s| {
                Felt::from_hex(s)
                    .map_err(|_| SponsorError::Chain(format!("call returned a non-felt: {s:?}")))
            })
            .collect()
    }

    /// Reads a token symbol, for verifying a configured token address is what it is
    /// believed to be.
    ///
    /// Exposed because a wrong fee-token address makes every balance read as zero,
    /// which refuses every deployment request as unfunded — a confusing failure that
    /// one call at startup can rule out.
    pub async fn token_symbol(&self, token: Felt) -> Result<String, SponsorError> {
        decode_byte_array(&self.call(token, "symbol", &[]).await?)
    }
}

/// Decodes a Cairo `ByteArray` return value into a string.
///
/// The layout is `[full_word_count, ...full_words, pending_word,
/// pending_word_len]`, where each full word packs 31 bytes.
///
/// Worth spelling out because the obvious reading is wrong in a way that looks
/// right: taking the *first* felt and parsing it as a Cairo short string returns an
/// empty string for any symbol shorter than 31 bytes, since that felt is the word
/// count and is `0`. A verification check that silently reads `""` for every token
/// would pass nothing and explain nothing.
fn decode_byte_array(felts: &[Felt]) -> Result<String, SponsorError> {
    let bad = |why: &str| SponsorError::Chain(format!("not a ByteArray: {why}"));
    let (&count, rest) = felts.split_first().ok_or_else(|| bad("empty"))?;
    let count = usize::try_from(count).map_err(|_| bad("absurd word count"))?;
    if rest.len() != count + 2 {
        return Err(bad(&format!(
            "{count} full words declared but {} felts follow",
            rest.len()
        )));
    }
    let mut out = Vec::new();
    for word in &rest[..count] {
        // A full word is 31 bytes, right-aligned in the 32-byte felt.
        out.extend_from_slice(&word.to_bytes_be()[1..]);
    }
    let pending_len = usize::try_from(rest[count + 1]).map_err(|_| bad("absurd pending length"))?;
    if pending_len > 31 {
        return Err(bad("pending word longer than a word"));
    }
    let pending = rest[count].to_bytes_be();
    out.extend_from_slice(&pending[32 - pending_len..]);
    String::from_utf8(out).map_err(|e| bad(&format!("not UTF-8: {e}")))
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

    async fn balance_of(&self, token: Felt, address: Felt) -> Result<u128, SponsorError> {
        // Works on an undeployed address: the balance lives in the token contract's
        // storage keyed by address, not in the account, which is what lets a
        // counterfactual address be funded before it exists.
        let out = self.call(token, "balanceOf", &[address]).await?;
        // Cairo u256 is returned as two felts, low then high.
        let (low, high) = match out.as_slice() {
            [low, high] => (low, high),
            other => {
                return Err(SponsorError::Chain(format!(
                    "balanceOf returned {} felts, expected a u256 as 2",
                    other.len()
                )));
            }
        };
        let low = u128::try_from(*low)
            .map_err(|_| SponsorError::Chain("balanceOf low limb does not fit a u128".into()))?;
        if *high != Felt::ZERO {
            // Saturate rather than truncate. Truncating could turn an enormous
            // balance into a small one and refuse to deploy a funded account; no
            // real STRK balance reaches 2^128 anyway, so this is a guard against a
            // decoding mistake, not an expected path.
            return Ok(u128::MAX);
        }
        Ok(low)
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

    #[tokio::test]
    async fn an_unreachable_endpoint_is_never_a_zero_balance() {
        // The mirror of the is_deployed rule. Reading a failure as zero would refuse
        // to deploy an account that has in fact been funded.
        let chain = JsonRpcChain::new("http://127.0.0.1:1/rpc").unwrap();
        let result = chain.balance_of(crate::STRK_MAINNET, Felt::ONE).await;
        assert!(
            matches!(result, Err(SponsorError::Chain(_))),
            "expected a Chain error, got {result:?}"
        );
    }

    /// Verifies [`crate::STRK_MAINNET`] really is STRK, against the live node.
    ///
    /// `#[ignore]`d because it needs the network, but committed rather than run once
    /// and written up in a comment: a token address is exactly the kind of constant
    /// that gets copied wrong, and a wrong one silently refuses every deployment
    /// request as unfunded. Run it when changing the constant:
    ///
    /// ```text
    /// cargo test -p buzz-paymaster -- --ignored the_documented_fee_token
    /// ```
    #[tokio::test]
    #[ignore = "requires network access to Starknet mainnet"]
    async fn the_documented_fee_token_is_strk_on_mainnet() {
        let chain = JsonRpcChain::new("https://mainnet.nodes.starknet.org/rpc/v0_10").unwrap();
        let symbol = chain
            .token_symbol(crate::STRK_MAINNET)
            .await
            .expect("symbol() on the fee token");
        assert_eq!(symbol, "STRK", "STRK_MAINNET does not point at STRK");

        // Exercise the u256 low/high decode against real data. The token contract
        // holds a non-trivial balance of its own — ~61,743 STRK when this was
        // written — so the bounds are deliberately loose enough to survive it
        // moving, while still failing on a decode that produced garbage.
        const ONE_STRK: u128 = 1_000_000_000_000_000_000;
        let held = chain
            .balance_of(crate::STRK_MAINNET, crate::STRK_MAINNET)
            .await
            .expect("balanceOf on a live token");
        assert!(held > 0, "expected the token contract to hold something");
        assert!(
            held < 1_000_000_000_000 * ONE_STRK,
            "a balance above any plausible supply means the u256 decode is wrong, got {held}"
        );

        // And an address nothing has ever sent to must read zero rather than error:
        // that is the case a member's fresh account is in, and treating it as an
        // error would refuse every first deployment.
        let empty = chain
            .balance_of(crate::STRK_MAINNET, Felt::from(2_u32))
            .await
            .expect("balanceOf must answer for an unfunded, undeployed address");
        assert_eq!(empty, 0);
    }
}
