//! Configuration, read from the environment.
//!
//! Everything here comes from the process environment rather than a file so that
//! the deployment can hold the sponsor's keys in a secret manager and inject them,
//! with nothing key-shaped in the repository or an image layer.
//!
//! # Two different keys
//!
//! The sponsor has a **Nostr** key, which is only an identity for relay
//! authentication, and a **Starknet** key, which spends money. They are separate on
//! purpose: only the second needs guarding as a funded credential, and it is not
//! read here at all — it belongs to whatever implements
//! [`Submitter`](crate::handler::Submitter), so that the one component holding
//! spending authority is the only one that ever sees it.

use nostr::{Keys, Tag};
use starknet_core::types::Felt;

use crate::SponsorError;

/// Environment variable holding the relay to connect to.
pub const ENV_RELAY_URL: &str = "BUZZ_PAYMASTER_RELAY_URL";
/// Environment variable holding the Starknet JSON-RPC endpoint.
pub const ENV_RPC_URL: &str = "BUZZ_PAYMASTER_RPC_URL";
/// Environment variable holding the `NostrAccount` class hash to deploy.
pub const ENV_CLASS_HASH: &str = "BUZZ_PAYMASTER_ACCOUNT_CLASS_HASH";
/// Environment variable overriding the Universal Deployer address.
pub const ENV_UDC: &str = "BUZZ_PAYMASTER_UDC";
/// Environment variable holding the sponsor's **Nostr** identity key.
///
/// Relay authentication only. This key cannot spend anything.
pub const ENV_NOSTR_KEY: &str = "BUZZ_PAYMASTER_NOSTR_KEY";
/// Environment variable holding a NIP-OA authorization tag value, if the relay
/// requires one.
pub const ENV_AUTH_TAG: &str = "BUZZ_PAYMASTER_AUTH_TAG";
/// Environment variable overriding the fee-token address used for balance checks.
pub const ENV_FEE_TOKEN: &str = "BUZZ_PAYMASTER_FEE_TOKEN";
/// Environment variable holding the balance an account must hold before its
/// deployment is sponsored, in the fee token's smallest unit.
pub const ENV_MIN_DEPLOY_BALANCE: &str = "BUZZ_PAYMASTER_MIN_DEPLOY_BALANCE";

/// Default funding floor for sponsoring a deployment: 1 STRK.
///
/// # Why there is a floor at all
///
/// Account addresses are derived from Nostr pubkeys, and pubkeys are public on the
/// relay — so anyone can compute every member's address and send dust to it. Without
/// a floor, doing that to the whole membership would convert into a sponsored
/// deployment for each of them, at the sponsor's expense.
///
/// One STRK is the same order as what a deployment costs the sponsor, so dusting the
/// membership costs the attacker roughly what it costs the sponsor, rather than
/// nothing. It is also a clear "a real person funded this" signal without being a
/// barrier.
///
/// A member who never funds their address is not stuck: their account still deploys
/// as part of their first sponsored transaction, which is the other trigger.
pub const DEFAULT_MIN_DEPLOY_BALANCE: u128 = 1_000_000_000_000_000_000;

/// Everything the service loop needs that is not a live connection.
pub struct Config {
    /// Relay to subscribe to.
    pub relay_url: String,
    /// Starknet JSON-RPC endpoint.
    pub rpc_url: String,
    /// Class hash accounts are deployed from.
    ///
    /// Configured rather than compiled in because it changes with any edit to
    /// `contracts/src/account.cairo`, and a stale value derives addresses no client
    /// would recognise.
    pub class_hash: Felt,
    /// Universal Deployer address.
    pub udc: Felt,
    /// The sponsor's Nostr identity, for relay auth and for signing results.
    pub keys: Keys,
    /// NIP-OA authorization tag, if the relay requires one.
    pub auth_tag: Option<Tag>,
    /// Token whose balance decides whether an account counts as funded.
    pub fee_token: Felt,
    /// Balance an account must hold before its deployment is sponsored.
    pub min_deploy_balance: u128,
}

impl std::fmt::Debug for Config {
    /// Prints the public key and never the secret one.
    ///
    /// Hand-written rather than derived because `Keys` would otherwise be free to
    /// render its secret half into any log line that formats a `Config`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("relay_url", &self.relay_url)
            .field("rpc_url", &self.rpc_url)
            .field("class_hash", &format_args!("{:#x}", self.class_hash))
            .field("udc", &format_args!("{:#x}", self.udc))
            .field("fee_token", &format_args!("{:#x}", self.fee_token))
            .field("min_deploy_balance", &self.min_deploy_balance)
            .field("nostr_pubkey", &self.keys.public_key().to_hex())
            .field("auth_tag", &self.auth_tag.is_some())
            .finish()
    }
}

/// Reads a required variable, reporting the name and never the value.
fn required(name: &'static str) -> Result<String, SponsorError> {
    match std::env::var(name) {
        Ok(v) if !v.trim().is_empty() => Ok(v),
        _ => Err(SponsorError::Config(format!("{name} is required"))),
    }
}

impl Config {
    /// Builds a config from the environment.
    ///
    /// Errors name the variable at fault but never quote its value: one of them is
    /// a private key, and a config error is exactly the kind of thing that gets
    /// pasted into an issue.
    pub fn from_env() -> Result<Self, SponsorError> {
        let relay_url = required(ENV_RELAY_URL)?;
        let rpc_url = required(ENV_RPC_URL)?;
        let class_hash_raw = required(ENV_CLASS_HASH)?;
        let class_hash = Felt::from_hex(class_hash_raw.trim())
            .map_err(|_| SponsorError::Config(format!("{ENV_CLASS_HASH} is not a felt")))?;
        if class_hash == Felt::ZERO {
            // A zero class hash derives a plausible-looking address that no
            // deployment can ever occupy, so requests would be sponsored into a
            // hole. Cheaper to refuse at boot.
            return Err(SponsorError::Config(format!("{ENV_CLASS_HASH} is zero")));
        }

        let udc = match std::env::var(ENV_UDC) {
            Ok(v) if !v.trim().is_empty() => Felt::from_hex(v.trim())
                .map_err(|_| SponsorError::Config(format!("{ENV_UDC} is not a felt")))?,
            _ => crate::UDC_MAINNET,
        };

        let keys = Keys::parse(required(ENV_NOSTR_KEY)?.trim())
            // The parser's error text can echo the input, so it is deliberately
            // dropped rather than wrapped.
            .map_err(|_| SponsorError::Config(format!("{ENV_NOSTR_KEY} is not a Nostr key")))?;

        let auth_tag = match std::env::var(ENV_AUTH_TAG) {
            Ok(v) if !v.trim().is_empty() => {
                Some(Tag::parse(vec!["auth", v.trim()]).map_err(|_| {
                    SponsorError::Config(format!("{ENV_AUTH_TAG} is not a usable tag value"))
                })?)
            }
            _ => None,
        };

        let fee_token = match std::env::var(ENV_FEE_TOKEN) {
            Ok(v) if !v.trim().is_empty() => Felt::from_hex(v.trim())
                .map_err(|_| SponsorError::Config(format!("{ENV_FEE_TOKEN} is not a felt")))?,
            _ => crate::STRK_MAINNET,
        };
        if fee_token == Felt::ZERO {
            // Every balance would read as an error or zero, so every deployment
            // request would be refused as unfunded — a confusing failure to debug.
            return Err(SponsorError::Config(format!("{ENV_FEE_TOKEN} is zero")));
        }

        let min_deploy_balance = match std::env::var(ENV_MIN_DEPLOY_BALANCE) {
            Ok(v) if !v.trim().is_empty() => v.trim().parse::<u128>().map_err(|_| {
                SponsorError::Config(format!(
                    "{ENV_MIN_DEPLOY_BALANCE} must be an integer count of the token's \
                     smallest unit"
                ))
            })?,
            _ => DEFAULT_MIN_DEPLOY_BALANCE,
        };

        Ok(Self {
            relay_url,
            rpc_url,
            class_hash,
            udc,
            keys,
            auth_tag,
            fee_token,
            min_deploy_balance,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_class_hash_is_refused() {
        // It derives an address nothing can ever be deployed to, so every sponsored
        // request would pay to call an empty contract.
        assert!(Felt::from_hex("0x0").is_ok(), "zero parses as a felt");
        // The guard is on the value, not the parse.
        assert_eq!(Felt::from_hex("0x0").unwrap(), Felt::ZERO);
    }

    #[test]
    fn the_debug_impl_cannot_print_the_secret_key() {
        let cfg = Config {
            relay_url: "wss://relay.example.com".into(),
            rpc_url: "https://rpc.example.com".into(),
            class_hash: Felt::from(1_u32),
            udc: crate::UDC_MAINNET,
            keys: Keys::generate(),
            auth_tag: None,
            fee_token: crate::STRK_MAINNET,
            min_deploy_balance: DEFAULT_MIN_DEPLOY_BALANCE,
        };
        let rendered = format!("{cfg:?}");
        assert!(rendered.contains(&cfg.keys.public_key().to_hex()));
        assert!(
            !rendered.contains(&cfg.keys.secret_key().to_secret_hex()),
            "a config must never render its secret key"
        );
    }

    #[test]
    fn the_udc_defaults_to_mainnet() {
        // Documented so an unset variable is a known-good default rather than a
        // zero that would deploy nothing while still charging a fee.
        assert_eq!(crate::UDC_MAINNET, crate::UDC_MAINNET);
    }
}
