//! NIP-SW attestation verification against a Starknet RPC endpoint.
//!
//! See `docs/nips/NIP-SW.md`. A conforming relay verifies a wallet binding's
//! attestation on-chain before storing the event, so a stored binding is an
//! attested one.
//!
//! # Two invariants this module exists to hold
//!
//! **The message hash is derived, never submitted.** It comes from
//! [`buzz_core::snip12::BindingMessage`], built from the event's own author
//! pubkey and payload fields. Trusting a submitted hash would be forgeable with
//! public data: Starknet transaction signatures are on-chain, so an attacker
//! could replay any `(tx_hash, signature)` pair from a victim's account under
//! their own Nostr identity and the account would confirm it valid.
//!
//! **The endpoint is operator-configured, never submitter-supplied.** Letting a
//! client name the RPC would let an attacker point verification at an endpoint
//! that answers `VALID` to everything.
//!
//! Verification fails **closed**: any transport error, timeout, missing
//! endpoint, or unparseable response rejects the event. Failing open would break
//! the one invariant relay-side verification exists to create.

use std::collections::HashMap;
use std::time::Duration;

use buzz_core::snip12::{BindingMessage, IS_VALID_SIGNATURE_SELECTOR};
use buzz_core::wallet_binding::WalletBinding;

/// Environment variable prefix for per-chain RPC endpoints.
///
/// `BUZZ_STARKNET_RPC_SN_MAIN=https://…` configures `SN_MAIN`.
pub const RPC_ENV_PREFIX: &str = "BUZZ_STARKNET_RPC_";

/// Default per-request timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// SNIP-6 `is_valid_signature` returns the `VALID` short string on success.
///
/// `0x56414c4944` is ASCII `VALID`.
const VALID_SHORT_STRING: &str = "0x56414c4944";

/// Errors from verifying a binding attestation. Every variant is a rejection.
#[derive(Debug, thiserror::Error)]
pub enum StarknetVerifyError {
    /// No RPC endpoint is configured for the binding's chain.
    #[error("no Starknet RPC endpoint configured for chain `{0}`")]
    NoEndpoint(String),
    /// The message hash could not be derived from the binding.
    #[error("could not derive attestation message: {0}")]
    Derive(#[from] buzz_core::snip12::Snip12Error),
    /// Transport failure, timeout, or non-success HTTP status.
    ///
    /// Fail-closed: this rejects the event rather than accepting it unverified.
    #[error("Starknet RPC call failed: {0}")]
    Rpc(String),
    /// The node returned a JSON-RPC error, or a response we could not read.
    #[error("Starknet RPC returned an error: {0}")]
    RpcError(String),
    /// The account contract did not confirm the signature.
    #[error("account {address} rejected the attestation signature")]
    SignatureRejected {
        /// The account that rejected it.
        address: String,
    },
}

/// Verifies binding attestations against per-chain Starknet RPC endpoints.
#[derive(Debug, Clone)]
pub struct StarknetVerifier {
    endpoints: HashMap<String, String>,
    client: reqwest::Client,
}

impl StarknetVerifier {
    /// Build a verifier from `BUZZ_STARKNET_RPC_<CHAIN_ID>` environment
    /// variables.
    ///
    /// A relay with no endpoints configured accepts no bindings — every
    /// verification returns [`StarknetVerifyError::NoEndpoint`]. That is the
    /// intended default: a relay that has not opted in must not store
    /// unverified bindings.
    #[must_use]
    pub fn from_env() -> Self {
        let endpoints = std::env::vars()
            .filter_map(|(key, value)| {
                let chain = key.strip_prefix(RPC_ENV_PREFIX)?;
                if chain.is_empty() || value.trim().is_empty() {
                    return None;
                }
                Some((chain.to_string(), value.trim().to_string()))
            })
            .collect();
        Self::with_endpoints(endpoints)
    }

    /// Build a verifier with explicit chain → endpoint mappings.
    #[must_use]
    pub fn with_endpoints(endpoints: HashMap<String, String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            // A default client is still usable; only the timeout is lost, and
            // per-request behaviour stays fail-closed either way.
            .unwrap_or_default();
        Self { endpoints, client }
    }

    /// Whether this relay is configured to verify bindings on `chain_id`.
    #[must_use]
    pub fn supports_chain(&self, chain_id: &str) -> bool {
        self.endpoints.contains_key(chain_id)
    }

    /// Chains this relay verifies, for capability advertisement.
    pub fn supported_chains(&self) -> impl Iterator<Item = &str> {
        self.endpoints.keys().map(String::as_str)
    }

    /// Verify a binding's attestation on-chain.
    ///
    /// `author_pubkey` MUST be the binding event's author (32-byte hex), not a
    /// value from the payload — it is what makes the attestation directional.
    ///
    /// Returns `Ok(())` only when the account contract confirms the signature
    /// over the *derived* message hash.
    pub async fn verify(
        &self,
        binding: &WalletBinding,
        author_pubkey: &str,
    ) -> Result<(), StarknetVerifyError> {
        let endpoint = self
            .endpoints
            .get(&binding.chain_id)
            .ok_or_else(|| StarknetVerifyError::NoEndpoint(binding.chain_id.clone()))?;

        let message = BindingMessage {
            nostr_pubkey: author_pubkey,
            chain_id: &binding.chain_id,
            signed_at: binding.attestation.signed_at,
            account_address: &binding.address,
        };
        let message_hash = message.message_hash_hex()?;

        // SNIP-6: is_valid_signature(hash: felt252, signature: Array<felt252>).
        // An Array is serialised as its length followed by its elements.
        let mut calldata = Vec::with_capacity(binding.attestation.signature.len() + 2);
        calldata.push(message_hash);
        calldata.push(format!("0x{:x}", binding.attestation.signature.len()));
        calldata.extend(binding.attestation.signature.iter().cloned());

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "starknet_call",
            "params": {
                "request": {
                    "contract_address": binding.address,
                    "entry_point_selector": IS_VALID_SIGNATURE_SELECTOR,
                    "calldata": calldata,
                },
                // `latest` is the newest block accepted on L2. Never `pending`:
                // verifying against unaccepted state could confirm a signature
                // the chain later discards.
                "block_id": "latest"
            }
        });

        let response = self
            .client
            .post(endpoint)
            .json(&request)
            .send()
            .await
            .map_err(|e| StarknetVerifyError::Rpc(e.to_string()))?;

        if !response.status().is_success() {
            return Err(StarknetVerifyError::Rpc(format!(
                "HTTP {}",
                response.status()
            )));
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| StarknetVerifyError::Rpc(e.to_string()))?;

        // A reverted call is the account saying "not valid", which is a
        // rejection rather than an infrastructure failure. Both reject; the
        // distinction only affects the log line an operator sees.
        if let Some(error) = body.get("error") {
            return Err(StarknetVerifyError::RpcError(error.to_string()));
        }

        let result = body
            .get("result")
            .and_then(|r| r.as_array())
            .ok_or_else(|| {
                StarknetVerifyError::RpcError("response had no result array".to_string())
            })?;

        if is_valid_result(result) {
            Ok(())
        } else {
            Err(StarknetVerifyError::SignatureRejected {
                address: binding.address.clone(),
            })
        }
    }
}

/// Whether an `is_valid_signature` return value means "valid".
///
/// SNIP-6 specifies the `VALID` short string. Cairo 0-era and some
/// SNIP-5-vintage accounts return `TRUE` (1) instead, and both are still
/// deployed, so accept either. Anything else — including `0` and an empty
/// return — is a rejection.
fn is_valid_result(result: &[serde_json::Value]) -> bool {
    let Some(first) = result.first().and_then(|v| v.as_str()) else {
        return false;
    };
    if felt_eq(first, VALID_SHORT_STRING) {
        return true;
    }
    felt_eq(first, "0x1")
}

/// Compare two felt-hex strings numerically, ignoring leading zeros and case.
fn felt_eq(a: &str, b: &str) -> bool {
    fn normalise(s: &str) -> String {
        s.strip_prefix("0x")
            .unwrap_or(s)
            .trim_start_matches('0')
            .to_ascii_lowercase()
    }
    normalise(a) == normalise(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_endpoints_means_no_supported_chains() {
        let v = StarknetVerifier::with_endpoints(HashMap::new());
        assert!(!v.supports_chain("SN_MAIN"));
        assert_eq!(v.supported_chains().count(), 0);
    }

    #[test]
    fn supports_only_configured_chains() {
        let mut endpoints = HashMap::new();
        endpoints.insert(
            "SN_SEPOLIA".to_string(),
            "https://example.invalid".to_string(),
        );
        let v = StarknetVerifier::with_endpoints(endpoints);
        assert!(v.supports_chain("SN_SEPOLIA"));
        assert!(!v.supports_chain("SN_MAIN"));
    }

    #[tokio::test]
    async fn unconfigured_chain_rejects_without_a_network_call() {
        // Fail-closed: an unconfigured chain must reject, not fall back to
        // storing the binding unverified.
        let v = StarknetVerifier::with_endpoints(HashMap::new());
        let binding = binding_fixture();
        let err = v
            .verify(&binding, &"11".repeat(32))
            .await
            .expect_err("must reject");
        assert!(matches!(err, StarknetVerifyError::NoEndpoint(c) if c == "SN_MAIN"));
    }

    #[test]
    fn valid_short_string_is_accepted() {
        assert!(is_valid_result(&[serde_json::json!("0x56414c4944")]));
        // Leading-zero and case variants of the same felt.
        assert!(is_valid_result(&[serde_json::json!("0x056414C4944")]));
    }

    #[test]
    fn legacy_true_is_accepted() {
        // Cairo 0-era accounts return TRUE rather than 'VALID'.
        assert!(is_valid_result(&[serde_json::json!("0x1")]));
        assert!(is_valid_result(&[serde_json::json!("0x0001")]));
    }

    #[test]
    fn zero_and_empty_are_rejected() {
        assert!(!is_valid_result(&[serde_json::json!("0x0")]));
        assert!(!is_valid_result(&[]));
        assert!(!is_valid_result(&[serde_json::json!("0x2")]));
        // A non-string entry must not be coerced into success.
        assert!(!is_valid_result(&[serde_json::json!(1)]));
    }

    fn binding_fixture() -> WalletBinding {
        use buzz_core::wallet_binding::{Attestation, AttestationScheme, SignerScheme};
        WalletBinding {
            address: "0x04a5".to_string(),
            chain_id: "SN_MAIN".to_string(),
            class_hash: None,
            signer_scheme: SignerScheme::Stark,
            attestation: Attestation {
                scheme: AttestationScheme::Snip12,
                signature: vec!["0xaa".to_string(), "0xbb".to_string()],
                signed_at: 1_785_400_000,
            },
        }
    }
}
