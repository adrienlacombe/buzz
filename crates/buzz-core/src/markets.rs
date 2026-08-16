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
pub const FACTORY: &str = "0x046b18bbc9b0de137e4f919100ee6b61bf37d345f8099ff7f982b7eaffcab62d";

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

/// Product AVNU SNIP-29 proxy host on `bitcoinmarkets.app`.
///
/// Deploy may set `AVNU_PROXY_URL` to this value, or rely on this public host.
/// **No localhost default** — loopback (`http://127.0.0.1:8788`) was local-only
/// and must not ship. `AVNU_API_KEY` stays server-side on the proxy only.
///
/// Health: `GET {AVNU_PROXY_URL}/health` →
/// `{"service":"buzz-avnu-proxy","status":"ok"}`.
pub const PRODUCT_AVNU_PROXY_URL: &str = "https://paymaster.bitcoinmarkets.app";

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
    /// `AVNU_PROXY_URL` pointed at loopback; public product host required.
    #[error(
        "AVNU_PROXY_URL must not be loopback; use https://paymaster.bitcoinmarkets.app \
         (required env / public host, no localhost default)"
    )]
    AvnuProxyUrlLoopback,
    /// `place_bet` call batch failed validation.
    #[error("place_bet call batch rejected: {0}")]
    InvalidBetBatch(String),
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

/// Whether betting is halted given Bitcoin tip `current_height`.
///
/// Height-based helper (not wall-clock): once the tip reaches
/// `next_retarget - 24`, the wallet refuses new bets until after the retarget.
///
/// Product path uses [`betting_halted_by_remaining_blocks`] from the live
/// mempool.space difficulty-adjustment signal only. Keep this height helper for
/// unit tests — do not use it as a live betting fallback when mempool is down.
#[must_use]
pub fn betting_halted(current_height: u64) -> bool {
    current_height >= halt_height(current_height)
}

/// Product halt signal: `remainingBlocks` from
/// `GET https://mempool.space/api/v1/difficulty-adjustment`.
///
/// Halt when `remaining_blocks <= 24` (same wallet-owned rule as
/// next-retarget − 24, expressed as blocks remaining).
#[must_use]
pub fn betting_halted_by_remaining_blocks(remaining_blocks: u64) -> bool {
    remaining_blocks <= HALT_BLOCKS_BEFORE_RETARGET
}

/// Keyring slot markets signing actually uses (human identity nsec only).
///
/// Callers must pass this (or another real slot name under test) into
/// [`assert_markets_signing_keyring`] — never skip the gate by asserting the
/// constant in isolation without covering real `agent:<pubkey>` inputs.
#[must_use]
pub fn markets_signing_keyring_name() -> &'static str {
    HUMAN_IDENTITY_KEYRING_NAME
}

/// Keyring names that may own a counterfactual Starknet `NostrAccount`.
///
/// Agent keys live in the same keyring service under `agent:<pubkey>` and must
/// **never** receive a derived account. Only the human `"identity"` entry is
/// eligible. Pass the **actual** keyring name of the keys about to sign.
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

/// Gate used by `fund_lightning` / `place_bet` before Starknet account derivation.
///
/// `keyring_name` must be the real secret-store slot for the keys about to
/// sign (e.g. `"identity"`, or `agent:<64-hex>` under test). Passing only the
/// [`HUMAN_IDENTITY_KEYRING_NAME`] constant at the call site without also
/// testing real agent slot names is not a gate.
pub fn assert_markets_signing_keyring(keyring_name: &str) -> Result<(), MarketsError> {
    assert_human_keyring_name(keyring_name)
}

/// Whether `keyring_name` is the human identity entry.
#[must_use]
pub fn is_human_keyring_name(name: &str) -> bool {
    assert_human_keyring_name(name).is_ok()
}

fn url_is_loopback(base: &str) -> bool {
    let lower = base.to_ascii_lowercase();
    lower.contains("127.0.0.1")
        || lower.contains("localhost")
        || lower.contains("[::1]")
        || lower.contains("0.0.0.0")
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
    if url_is_loopback(base) {
        return Err(MarketsError::IndexerUrlLoopback);
    }
    Ok(base.to_string())
}

/// Resolve the AVNU SNIP-29 proxy base URL from `AVNU_PROXY_URL`.
///
/// Required env, or the product public host [`PRODUCT_AVNU_PROXY_URL`].
/// Refuses loopback — no `http://127.0.0.1:8788` default.
pub fn resolve_avnu_proxy_url() -> Result<String, MarketsError> {
    resolve_avnu_proxy_url_from(std::env::var("AVNU_PROXY_URL").ok().as_deref())
}

/// Resolve from an optional raw env value (tests / callers that already read env).
pub fn resolve_avnu_proxy_url_from(raw: Option<&str>) -> Result<String, MarketsError> {
    let chosen = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(PRODUCT_AVNU_PROXY_URL);
    let base = chosen.trim_end_matches('/');
    if url_is_loopback(base) {
        return Err(MarketsError::AvnuProxyUrlLoopback);
    }
    Ok(base.to_string())
}

/// One Starknet call as hex strings (frontend / JSON-RPC shape).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BetCallHex {
    /// Contract address (`0x…`).
    pub contract_address: String,
    /// Cairo entrypoint name.
    pub entrypoint: String,
    /// Calldata felts as hex strings.
    pub calldata: Vec<String>,
}

/// Compare two felt hex strings for equality (padding-insensitive).
pub fn felt_hex_eq(a: &str, b: &str) -> bool {
    match (Felt::from_hex(a.trim()), Felt::from_hex(b.trim())) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn felt_hex_canonical(value: &str) -> Result<String, MarketsError> {
    Felt::from_hex(value.trim())
        .map(|f| f.to_fixed_hex_string())
        .map_err(|_| MarketsError::InvalidBetBatch(format!("invalid felt hex: {value}")))
}

fn entrypoint_forbidden(entrypoint: &str) -> bool {
    let ep = entrypoint.to_ascii_lowercase();
    ep.contains("lightning") || ep.contains("atomiq") || ep.contains("invoice")
}

/// Build the wallet-fee `transfer` call Rust will put first in the signed batch.
#[must_use]
pub fn build_wallet_fee_call(token_amount: u128) -> BetCallHex {
    let fee = wallet_fee_amount(token_amount);
    let (low, high) = u256_felts(fee);
    BetCallHex {
        contract_address: COLLATERAL_TOKEN.to_string(),
        entrypoint: "transfer".to_string(),
        calldata: vec![
            FEE_RECIPIENT.to_string(),
            low.to_fixed_hex_string(),
            high.to_fixed_hex_string(),
        ],
    }
}

/// Rebuild + validate the `place_bet` batch.
///
/// Rust **owns** the wallet fee transfer: any frontend `transfer` is stripped
/// and replaced with [`build_wallet_fee_call`]. Remaining calls must be exactly
/// `approve` on [`COLLATERAL_TOKEN`] (spender [`DIFFICULTY_MARKET`]) and
/// `execute_trade` on [`DIFFICULTY_MARKET`]. Anything else is rejected.
///
/// Returns `(calls, fee_amount)` where `calls[0]` is always the fee transfer.
pub fn build_validated_bet_batch(
    calls: &[BetCallHex],
    token_amount: u128,
) -> Result<(Vec<BetCallHex>, u128), MarketsError> {
    if calls.is_empty() {
        return Err(MarketsError::InvalidBetBatch(
            "place_bet requires approve + execute_trade".into(),
        ));
    }

    let mut approve: Option<BetCallHex> = None;
    let mut trade: Option<BetCallHex> = None;

    for c in calls {
        if entrypoint_forbidden(&c.entrypoint) {
            return Err(MarketsError::InvalidBetBatch(
                "place_bet is Starknet-only; Lightning belongs on Fund".into(),
            ));
        }
        let ep = c.entrypoint.as_str();
        if felt_hex_eq(&c.contract_address, COLLATERAL_TOKEN) && ep.eq_ignore_ascii_case("transfer")
        {
            // Frontend may prepend a fee call; Rust rebuilds it — do not trust it.
            continue;
        }
        if felt_hex_eq(&c.contract_address, COLLATERAL_TOKEN) && ep.eq_ignore_ascii_case("approve")
        {
            if approve.is_some() {
                return Err(MarketsError::InvalidBetBatch("duplicate approve".into()));
            }
            if c.calldata.is_empty() || !felt_hex_eq(c.calldata[0].as_str(), DIFFICULTY_MARKET) {
                return Err(MarketsError::InvalidBetBatch(
                    "approve spender must be the difficulty market".into(),
                ));
            }
            approve = Some(BetCallHex {
                contract_address: felt_hex_canonical(&c.contract_address)?,
                entrypoint: "approve".to_string(),
                calldata: c
                    .calldata
                    .iter()
                    .map(|x| felt_hex_canonical(x))
                    .collect::<Result<Vec<_>, _>>()?,
            });
            continue;
        }
        if felt_hex_eq(&c.contract_address, DIFFICULTY_MARKET)
            && ep.eq_ignore_ascii_case("execute_trade")
        {
            if trade.is_some() {
                return Err(MarketsError::InvalidBetBatch(
                    "duplicate execute_trade".into(),
                ));
            }
            trade = Some(BetCallHex {
                contract_address: felt_hex_canonical(&c.contract_address)?,
                entrypoint: "execute_trade".to_string(),
                calldata: c
                    .calldata
                    .iter()
                    .map(|x| felt_hex_canonical(x))
                    .collect::<Result<Vec<_>, _>>()?,
            });
            continue;
        }
        return Err(MarketsError::InvalidBetBatch(format!(
            "unexpected call {} on {}",
            c.entrypoint, c.contract_address
        )));
    }

    let approve = approve.ok_or_else(|| {
        MarketsError::InvalidBetBatch("missing approve on collateral token".into())
    })?;
    let trade = trade.ok_or_else(|| {
        MarketsError::InvalidBetBatch("missing execute_trade on difficulty market".into())
    })?;

    let fee_amount = wallet_fee_amount(token_amount);
    let fee_call = build_wallet_fee_call(token_amount);
    Ok((vec![fee_call, approve, trade], fee_amount))
}

/// Whether the first signed call is the wallet fee transfer for `fee_amount`.
///
/// `PlaceBetResult.fee_amount` alone is not enough — the bytes about to be
/// BIP-340-signed must start with `transfer(FEE_RECIPIENT, fee)`.
pub fn fee_transfer_matches(
    contract_address: &str,
    entrypoint: &str,
    calldata: &[String],
    fee_amount: u128,
) -> bool {
    if !entrypoint.eq_ignore_ascii_case("transfer") {
        return false;
    }
    if !felt_hex_eq(contract_address, COLLATERAL_TOKEN) {
        return false;
    }
    if calldata.len() != 3 {
        return false;
    }
    let (low, high) = u256_felts(fee_amount);
    felt_hex_eq(&calldata[0], FEE_RECIPIENT)
        && felt_hex_eq(&calldata[1], &low.to_fixed_hex_string())
        && felt_hex_eq(&calldata[2], &high.to_fixed_hex_string())
}

/// Assert the first call in a signed batch is the wallet fee transfer.
pub fn assert_fee_is_first_call(
    contract_address: &str,
    entrypoint: &str,
    calldata: &[String],
    fee_amount: u128,
) -> Result<(), MarketsError> {
    if fee_transfer_matches(contract_address, entrypoint, calldata, fee_amount) {
        Ok(())
    } else {
        Err(MarketsError::InvalidBetBatch(
            "signed batch must start with wallet fee transfer to FEE_RECIPIENT".into(),
        ))
    }
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
    fn remaining_blocks_signal_halts_at_24() {
        assert!(!betting_halted_by_remaining_blocks(25));
        assert!(betting_halted_by_remaining_blocks(24));
        assert!(betting_halted_by_remaining_blocks(1));
        assert!(betting_halted_by_remaining_blocks(0));
    }

    #[test]
    fn human_identity_may_own_account() {
        assert!(is_human_keyring_name("identity"));
        assert_eq!(assert_human_keyring_name("identity"), Ok(()));
        assert_eq!(
            assert_markets_signing_keyring(markets_signing_keyring_name()),
            Ok(())
        );
    }

    #[test]
    fn agent_keys_do_not_get_starknet_accounts() {
        // Real secret_store / managed-agent slot names — not the identity constant.
        let agent = "agent:8dae5a92916c512029ad1534fcf264e0e2e33ce492acf34588bc6268f7570dd5";
        assert!(!is_human_keyring_name(agent));
        assert_eq!(
            assert_markets_signing_keyring(agent),
            Err(MarketsError::AgentKeyNotAllowed)
        );
        assert_eq!(
            assert_markets_signing_keyring("agent:abc123"),
            Err(MarketsError::AgentKeyNotAllowed)
        );
        assert_eq!(
            assert_markets_signing_keyring("agent:anything"),
            Err(MarketsError::AgentKeyNotAllowed)
        );
        // Random non-identity names are also refused.
        assert_eq!(
            assert_markets_signing_keyring("some-other-slot"),
            Err(MarketsError::AgentKeyNotAllowed)
        );
    }

    #[test]
    fn indexer_url_is_product_host_never_loopback() {
        assert_eq!(PRODUCT_INDEXER_URL, "https://markets.bitcoinmarkets.app");
        assert!(PRODUCT_INDEXER_URL.starts_with("https://"));
        assert!(!PRODUCT_INDEXER_URL.contains("127.0.0.1"));
        assert!(!PRODUCT_INDEXER_URL.contains("localhost"));
        assert_eq!(resolve_indexer_url_from(None).unwrap(), PRODUCT_INDEXER_URL);
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

    #[test]
    fn avnu_proxy_url_is_product_host_never_loopback() {
        assert_eq!(
            PRODUCT_AVNU_PROXY_URL,
            "https://paymaster.bitcoinmarkets.app"
        );
        assert!(PRODUCT_AVNU_PROXY_URL.starts_with("https://"));
        assert!(!PRODUCT_AVNU_PROXY_URL.contains("127.0.0.1"));
        assert!(!PRODUCT_AVNU_PROXY_URL.contains("localhost"));
        assert_eq!(
            resolve_avnu_proxy_url_from(None).unwrap(),
            PRODUCT_AVNU_PROXY_URL
        );
        assert_eq!(
            resolve_avnu_proxy_url_from(Some("")).unwrap(),
            PRODUCT_AVNU_PROXY_URL
        );
        assert_eq!(
            resolve_avnu_proxy_url_from(Some("https://paymaster.bitcoinmarkets.app/")).unwrap(),
            "https://paymaster.bitcoinmarkets.app"
        );
        assert_eq!(
            resolve_avnu_proxy_url_from(Some("http://127.0.0.1:8788")),
            Err(MarketsError::AvnuProxyUrlLoopback)
        );
        assert_eq!(
            resolve_avnu_proxy_url_from(Some("http://localhost:8788")),
            Err(MarketsError::AvnuProxyUrlLoopback)
        );
        assert_eq!(
            resolve_avnu_proxy_url_from(Some("https://paymaster.example/proxy/")).unwrap(),
            "https://paymaster.example/proxy"
        );
    }

    #[test]
    fn validated_bet_batch_rebuilds_fee_first() {
        let token_amount = 1_000_000u128;
        let approve = BetCallHex {
            contract_address: COLLATERAL_TOKEN.to_string(),
            entrypoint: "approve".into(),
            calldata: vec![DIFFICULTY_MARKET.to_string(), "0x1".into(), "0x0".into()],
        };
        let trade = BetCallHex {
            contract_address: DIFFICULTY_MARKET.to_string(),
            entrypoint: "execute_trade".into(),
            calldata: vec!["0xabc".into()],
        };
        // Frontend may send a wrong fee transfer — Rust must replace it.
        let bogus_fee = BetCallHex {
            contract_address: COLLATERAL_TOKEN.to_string(),
            entrypoint: "transfer".into(),
            calldata: vec!["0x1".into(), "0x1".into(), "0x0".into()],
        };
        let (batch, fee) =
            build_validated_bet_batch(&[bogus_fee, approve.clone(), trade.clone()], token_amount)
                .unwrap();
        assert_eq!(fee, wallet_fee_amount(token_amount));
        assert_eq!(batch.len(), 3);
        assert_eq!(batch[0].entrypoint, "transfer");
        assert!(fee_transfer_matches(
            &batch[0].contract_address,
            &batch[0].entrypoint,
            &batch[0].calldata,
            fee
        ));
        assert_eq!(batch[1].entrypoint, "approve");
        assert_eq!(batch[2].entrypoint, "execute_trade");
        assert_fee_is_first_call(
            &batch[0].contract_address,
            &batch[0].entrypoint,
            &batch[0].calldata,
            fee,
        )
        .unwrap();
    }

    #[test]
    fn validated_bet_batch_rejects_foreign_contracts() {
        let bad = BetCallHex {
            contract_address: "0xdead".into(),
            entrypoint: "execute_trade".into(),
            calldata: vec![],
        };
        assert!(matches!(
            build_validated_bet_batch(&[bad], 1000),
            Err(MarketsError::InvalidBetBatch(_))
        ));
    }
}
