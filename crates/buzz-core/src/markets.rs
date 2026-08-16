//! Bitcoin Markets product helpers: wallet fee, retarget halt, and human-only
//! Starknet account ownership.
//!
//! The UI never surfaces L2 vocabulary. These helpers keep the wallet-side
//! invariants correct regardless of how the frontend is framed.

use starknet_crypto::Felt;

/// Wallet fee in basis points charged on each bet (`ceil(amount * bps / 10_000)`).
pub const WALLET_FEE_BPS: u128 = 10;

/// Fee recipient for the wallet fee transfer (mainnet).
pub const FEE_RECIPIENT: &str =
    "0x03df153485c79b693c42563d71abd315635a4819ba3415d07f8421b4ebc839c6";

/// strkBTC collateral token (8 decimals). Product copy calls this “BTC”.
pub const COLLATERAL_TOKEN: &str =
    "0x0787150e306e6eae6e3f79dea881770e8bbff2c1b8eb490f969669ee945b3135";

/// Live Bitcoin difficulty market (LOGNORMAL family, raw difficulty axis).
pub const DIFFICULTY_MARKET: &str =
    "0x023b3a7bbe48a905ceadc17cd21b6b71fedaf90ee1218e462b106e01703b9cc8";

/// Distribution factory (mainnet).
pub const FACTORY: &str =
    "0x046b18bbc9b0de137e4f919100ee6b61bf37d345f8099ff7f982b7eaffcab62d";

/// Declared `NostrAccount` class hash (mainnet).
pub const NOSTR_ACCOUNT_CLASS_HASH: &str =
    "0x0414f62ea1ed35f8c7bd3b794d94efc95e01bccf04e0f47211fc198f7f56f537";

/// Protocol floor for a trade, in raw 8-decimal units (~0.000977 BTC).
pub const MIN_TRADE_RAW: u128 = 97_700;

/// Bitcoin difficulty retarget interval.
pub const RETARGET_INTERVAL: u64 = 2016;

/// Halt betting this many blocks before the next retarget height.
pub const HALT_BLOCKS_BEFORE_RETARGET: u64 = 24;

/// Product indexer host on `bitcoinmarkets.app`.
///
/// Deploy must set `INDEXER_URL` to this value (or rely on this public host).
/// **No localhost default** — Adrien does not want the indexer run locally.
/// Loopback (`http://127.0.0.1:8787`) was listing-proof only and must not ship.
///
/// Listing endpoints (no auth; never use indexer `ADMIN_API_KEY` here):
/// - `GET {INDEXER_URL}/api/markets`
/// - `GET {INDEXER_URL}/health`
pub const PRODUCT_INDEXER_URL: &str = "https://markets.bitcoinmarkets.app";

/// Keyring entry name for the human identity nsec.
pub const HUMAN_IDENTITY_KEYRING_NAME: &str = "identity";

/// Errors from markets helpers.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MarketsError {
    /// An agent (or otherwise non-human) key was asked to own a Starknet account.
    #[error("only the human identity may own a Starknet account; agent keys are excluded")]
    AgentKeyNotAllowed,
    /// A keyring name was empty or malformed.
    #[error("invalid keyring name: {0}")]
    InvalidKeyringName(String),
    /// `INDEXER_URL` pointed at loopback; public product host required.
    #[error(
        "INDEXER_URL must not be loopback; use https://markets.bitcoinmarkets.app \
         (required env / public host, no localhost default)"
    )]
    IndexerUrlLoopback,
}

/// Computes the wallet fee for a trade collateral amount.
///
/// `fee = ceil(token_amount * WALLET_FEE_BPS / 10_000)`, floored at **1 sat**
/// (raw unit) when the product would otherwise be zero. The fee is a separate
/// `transfer` call and must **not** bump `approve` or `supplied_collateral`.
#[must_use]
pub fn wallet_fee_amount(token_amount: u128) -> u128 {
    if token_amount == 0 {
        return 0;
    }
    // ceil(a * bps / 10_000) = (a * bps + 9999) / 10_000
    let fee = token_amount
        .saturating_mul(WALLET_FEE_BPS)
        .saturating_add(9_999)
        / 10_000;
    fee.max(1)
}

/// Splits a raw u256 amount into Starknet `(low, high)` felts for calldata.
#[must_use]
pub fn u256_felts(amount: u128) -> (Felt, Felt) {
    (Felt::from(amount), Felt::ZERO)
}

/// Next Bitcoin difficulty retarget height strictly after `current_height`.
///
/// Retargets occur at multiples of [`RETARGET_INTERVAL`]. When `current_height`
/// is itself a multiple, the *next* retarget is one interval ahead.
#[must_use]
pub fn next_retarget_height(current_height: u64) -> u64 {
    let completed = current_height / RETARGET_INTERVAL;
    (completed + 1) * RETARGET_INTERVAL
}

/// Height at which betting must halt (inclusive): 24 blocks before the next
/// retarget.
#[must_use]
pub fn halt_height(current_height: u64) -> u64 {
    next_retarget_height(current_height).saturating_sub(HALT_BLOCKS_BEFORE_RETARGET)
}

/// Whether betting is halted at `current_height`.
///
/// Halt is height-based, not wall-clock: once the tip reaches
/// `next_retarget - 24`, the wallet refuses new bets until after the retarget.
#[must_use]
pub fn betting_halted(current_height: u64) -> bool {
    current_height >= halt_height(current_height)
}

/// Keyring names that may own a counterfactual Starknet `NostrAccount`.
///
/// Agent keys live in the same keyring service under `agent:<pubkey>` and must
/// **never** receive a derived account. Only the human `"identity"` entry (or an
/// explicit human pubkey passed by the signing path) is eligible.
pub fn assert_human_keyring_name(name: &str) -> Result<(), MarketsError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(MarketsError::InvalidKeyringName(name.to_string()));
    }
    if trimmed == HUMAN_IDENTITY_KEYRING_NAME {
        return Ok(());
    }
    if trimmed.starts_with("agent:") {
        return Err(MarketsError::AgentKeyNotAllowed);
    }
    // Anything else is not the human identity slot.
    Err(MarketsError::AgentKeyNotAllowed)
}

/// Whether `keyring_name` is the human identity entry.
#[must_use]
pub fn is_human_keyring_name(name: &str) -> bool {
    assert_human_keyring_name(name).is_ok()
}

/// Resolve the markets indexer base URL from `INDEXER_URL`.
///
/// Required env, or the product public host [`PRODUCT_INDEXER_URL`].
/// Refuses loopback — no `http://127.0.0.1:8787` default.
pub fn resolve_indexer_url() -> Result<String, MarketsError> {
    resolve_indexer_url_from(std::env::var("INDEXER_URL").ok().as_deref())
}

/// Resolve from an optional raw env value (tests / callers that already read env).
pub fn resolve_indexer_url_from(raw: Option<&str>) -> Result<String, MarketsError> {
    let chosen = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(PRODUCT_INDEXER_URL);
    let base = chosen.trim_end_matches('/');
    if base.contains("127.0.0.1") || base.to_ascii_lowercase().contains("localhost") {
        return Err(MarketsError::IndexerUrlLoopback);
    }
    Ok(base.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fee_is_10_bps_ceil() {
        // 10_000 raw * 10 / 10_000 = 10
        assert_eq!(wallet_fee_amount(10_000), 10);
        // 1_000 * 10 / 10_000 = 1 exactly
        assert_eq!(wallet_fee_amount(1_000), 1);
        // 999 * 10 / 10_000 = 0.999 → ceil → 1 (min sat)
        assert_eq!(wallet_fee_amount(999), 1);
        // 100 * 10 / 10_000 = 0.1 → ceil → 1
        assert_eq!(wallet_fee_amount(100), 1);
        // Large amount
        assert_eq!(wallet_fee_amount(2_000_000), 2_000);
    }

    #[test]
    fn fee_min_is_one_sat_when_product_would_be_zero() {
        assert_eq!(wallet_fee_amount(1), 1);
        assert_eq!(wallet_fee_amount(50), 1);
    }

    #[test]
    fn fee_zero_amount_is_zero() {
        assert_eq!(wallet_fee_amount(0), 0);
    }

    #[test]
    fn next_retarget_after_genesis_era() {
        assert_eq!(next_retarget_height(0), 2016);
        assert_eq!(next_retarget_height(1), 2016);
        assert_eq!(next_retarget_height(2015), 2016);
        assert_eq!(next_retarget_height(2016), 4032);
        assert_eq!(next_retarget_height(2017), 4032);
    }

    #[test]
    fn halt_is_24_blocks_before_retarget() {
        // Tip in the first era: halt at 2016 - 24 = 1992
        assert_eq!(halt_height(1000), 1992);
        assert!(!betting_halted(1991));
        assert!(betting_halted(1992));
        assert!(betting_halted(2015));
        // After retarget at 2016, next halt is 4032 - 24 = 4008
        assert!(!betting_halted(2016));
        assert_eq!(halt_height(2016), 4008);
        assert!(betting_halted(4008));
    }

    #[test]
    fn human_identity_may_own_account() {
        assert!(is_human_keyring_name("identity"));
        assert_eq!(assert_human_keyring_name("identity"), Ok(()));
    }

    #[test]
    fn agent_keys_do_not_get_starknet_accounts() {
        let agent = "agent:8dae5a92916c512029ad1534fcf264e0e2e33ce492acf34588bc6268f7570dd5";
        assert!(!is_human_keyring_name(agent));
        assert_eq!(
            assert_human_keyring_name(agent),
            Err(MarketsError::AgentKeyNotAllowed)
        );
        assert_eq!(
            assert_human_keyring_name("agent:anything"),
            Err(MarketsError::AgentKeyNotAllowed)
        );
        // Random non-identity names are also refused.
        assert_eq!(
            assert_human_keyring_name("some-other-slot"),
            Err(MarketsError::AgentKeyNotAllowed)
        );
    }

    #[test]
    fn indexer_url_is_product_host_never_loopback() {
        assert_eq!(PRODUCT_INDEXER_URL, "https://markets.bitcoinmarkets.app");
        assert!(PRODUCT_INDEXER_URL.starts_with("https://"));
        assert!(!PRODUCT_INDEXER_URL.contains("127.0.0.1"));
        assert!(!PRODUCT_INDEXER_URL.contains("localhost"));
        assert_eq!(
            resolve_indexer_url_from(None).unwrap(),
            PRODUCT_INDEXER_URL
        );
        assert_eq!(
            resolve_indexer_url_from(Some("")).unwrap(),
            PRODUCT_INDEXER_URL
        );
        assert_eq!(
            resolve_indexer_url_from(Some("https://markets.bitcoinmarkets.app/")).unwrap(),
            "https://markets.bitcoinmarkets.app"
        );
        assert_eq!(
            resolve_indexer_url_from(Some("http://127.0.0.1:8787")),
            Err(MarketsError::IndexerUrlLoopback)
        );
        assert_eq!(
            resolve_indexer_url_from(Some("http://localhost:8787")),
            Err(MarketsError::IndexerUrlLoopback)
        );
    }
}
