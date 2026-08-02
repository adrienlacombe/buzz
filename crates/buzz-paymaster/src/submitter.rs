//! The one component holding spending authority.
//!
//! Everything else in this crate decides *whether* to pay. This decides *how much*,
//! and it is the only place a funded key exists.
//!
//! # Estimation is the guard, not just a price check
//!
//! `starknet_estimateFee` runs the whole multicall against the node, including
//! `__validate__` on the sponsor and `execute_from_outside_v2` on the user's account,
//! and errors if any of it reverts. So estimating **is** a dry run: a request with a
//! bad BIP-340 signature, a closed validity window, a spent nonce, or a call that
//! reverts is refused for free instead of costing a reverted transaction's fee.
//!
//! Without that, any member could drain the sponsor by publishing requests that were
//! never going to succeed — there is no per-member quota, by decision, so this is
//! what stands in its place.
//!
//! # And a ceiling on top of it
//!
//! The calls in a request are the *user's*, and arbitrary. A member could ask the
//! sponsor to pay for something enormous, and estimation would happily report a
//! correct, enormous number. [`ENV_MAX_FEE_FRI`] bounds what a single transaction can
//! cost; over it, the request is refused.
//!
//! # Submissions must not overlap
//!
//! The nonce is read from the pre-confirmed block and consumed by the transaction
//! that follows. Two submissions in flight would take the same nonce and the second
//! would be rejected — the reason [`crate::service`] services requests one at a time,
//! and the reason to run a single instance.

use starknet_accounts::{Account, ConnectedAccount, ExecutionEncoding, SingleOwnerAccount};
use starknet_core::types::{Call, Felt};
use starknet_providers::jsonrpc::HttpTransport;
use starknet_providers::{JsonRpcClient, Provider};
use starknet_signers::{LocalWallet, SigningKey};
use tracing::{info, warn};

use crate::handler::Submitter;
use crate::SponsorError;

/// Environment variable holding the sponsor's funded Starknet account address.
pub const ENV_STARKNET_ADDRESS: &str = "BUZZ_PAYMASTER_STARKNET_ADDRESS";
/// Environment variable holding the sponsor's Starknet signing key.
///
/// This is the credential that spends money. It is read once at construction and
/// handed straight to the signer; nothing else in this crate sees it, and no error
/// path formats it.
pub const ENV_STARKNET_KEY: &str = "BUZZ_PAYMASTER_STARKNET_KEY";
/// Environment variable holding the per-transaction fee ceiling, in Fri.
pub const ENV_MAX_FEE_FRI: &str = "BUZZ_PAYMASTER_MAX_FEE_FRI";

/// Default per-transaction ceiling: 10 STRK.
///
/// Chosen against measured cost rather than picked: a deploy-plus-execute estimated
/// at ~0.92 STRK on mainnet, and the bounds below pad that by 2.25x, so a legitimate
/// transaction lands near 2.1 STRK. Ten leaves room for a heavier user call while
/// still capping a runaway at a tenth of what an unbounded one could take. See
/// `contracts/DEPLOYMENTS.md` for the measurements.
pub const DEFAULT_MAX_FEE_FRI: u128 = 10_000_000_000_000_000_000;

/// Numerator of the padding applied to estimated gas and prices.
pub const BOUND_NUMERATOR: u128 = 3;
/// Denominator of the padding applied to estimated gas and prices.
///
/// 3/2 on both gas *and* price, so the worst-case fee is 2.25x the estimate. Padding
/// is necessary — gas prices move between estimation and inclusion, and a bound below
/// the actual cost means the transaction is rejected after the sponsor has already
/// decided to pay for it — but it is also the reason the ceiling is checked against
/// the padded bound rather than the raw estimate.
pub const BOUND_DENOMINATOR: u128 = 2;

/// Tip offered, in Fri per unit of L2 gas.
///
/// Zero, deliberately. A tip is unbounded extra fee outside the gas bounds, which
/// would make the ceiling meaningless, and the sponsor is not competing for
/// priority: request windows are minutes, not blocks. Revisit only if inclusion
/// actually starts failing under congestion.
pub const TIP: u64 = 0;

/// Reads a required variable, reporting the name and never the value.
fn required(name: &'static str) -> Result<String, SponsorError> {
    match std::env::var(name) {
        Ok(v) if !v.trim().is_empty() => Ok(v),
        _ => Err(SponsorError::Config(format!("{name} is required"))),
    }
}

/// Pads an estimate into a bound, saturating rather than wrapping.
fn pad(value: u128) -> u128 {
    value
        .saturating_mul(BOUND_NUMERATOR)
        .saturating_div(BOUND_DENOMINATOR)
}

/// Submits from a funded Starknet account with a locally held key.
pub struct LocalSubmitter {
    account: SingleOwnerAccount<JsonRpcClient<HttpTransport>, LocalWallet>,
    max_fee_fri: u128,
}

impl LocalSubmitter {
    /// Builds a submitter from the environment.
    ///
    /// Reads the chain id from the node rather than taking it as configuration: a
    /// configured value that disagreed with the node would produce signatures valid
    /// for a different chain, which the sequencer rejects with nothing to point at
    /// the cause.
    ///
    /// Returns the chain id alongside the submitter so the caller can hand the same
    /// value to [`crate::ChainConfig`] — the request-side chain check and the
    /// signing-side chain id must be the same number or the check guards nothing.
    pub async fn from_env(rpc_url: &str) -> Result<(Self, Felt), SponsorError> {
        let address = Felt::from_hex(required(ENV_STARKNET_ADDRESS)?.trim())
            .map_err(|_| SponsorError::Config(format!("{ENV_STARKNET_ADDRESS} is not a felt")))?;
        if address == Felt::ZERO {
            return Err(SponsorError::Config(format!(
                "{ENV_STARKNET_ADDRESS} is zero"
            )));
        }

        let max_fee_fri = match std::env::var(ENV_MAX_FEE_FRI) {
            Ok(v) if !v.trim().is_empty() => v.trim().parse::<u128>().map_err(|_| {
                SponsorError::Config(format!("{ENV_MAX_FEE_FRI} must be an integer count of Fri"))
            })?,
            _ => DEFAULT_MAX_FEE_FRI,
        };
        if max_fee_fri == 0 {
            // Zero would refuse every request while looking configured, which is a
            // worse failure than refusing to start.
            return Err(SponsorError::Config(format!(
                "{ENV_MAX_FEE_FRI} is zero; nothing could ever be sponsored"
            )));
        }

        let url = url::Url::parse(rpc_url)
            .map_err(|e| SponsorError::Config(format!("invalid RPC url: {e}")))?;
        let provider = JsonRpcClient::new(HttpTransport::new(url));
        let chain_id = provider
            .chain_id()
            .await
            .map_err(|e| SponsorError::Chain(format!("could not read chain id: {e}")))?;

        // The one line that touches the key. Parsed straight into the signer; the
        // error deliberately drops the parser's message, which can echo the input.
        let signer = LocalWallet::from_signing_key(SigningKey::from_secret_scalar(
            Felt::from_hex(required(ENV_STARKNET_KEY)?.trim())
                .map_err(|_| SponsorError::Config(format!("{ENV_STARKNET_KEY} is not a felt")))?,
        ));

        let account = SingleOwnerAccount::new(
            provider,
            signer,
            address,
            chain_id,
            // SNIP-6 calldata encoding. Every account deployed in the last several
            // years uses it; `Legacy` is Cairo 0. Getting this wrong fails on the
            // first submission rather than silently, so it is not configurable.
            ExecutionEncoding::New,
        );

        info!(
            sponsor_address = %address.to_fixed_hex_string(),
            chain_id = %chain_id.to_fixed_hex_string(),
            max_fee_fri,
            "sponsor account ready"
        );
        Ok((
            Self {
                account,
                max_fee_fri,
            },
            chain_id,
        ))
    }

    /// The sponsor's account address.
    pub fn address(&self) -> Felt {
        self.account.address()
    }

    /// The per-transaction ceiling, in Fri.
    pub fn max_fee_fri(&self) -> u128 {
        self.max_fee_fri
    }
}

impl Submitter for LocalSubmitter {
    async fn submit(&self, calls: Vec<Call>) -> Result<Felt, SponsorError> {
        // One nonce for both the estimate and the send. Resolving it twice would let
        // it move in between, and the transaction would be signed against a nonce the
        // sequencer no longer expects.
        let nonce = self
            .account
            .get_nonce()
            .await
            .map_err(|e| SponsorError::Chain(format!("could not read sponsor nonce: {e}")))?;

        let execution = self.account.execute_v3(calls).nonce(nonce).tip(TIP);

        // The dry run. An error here means the multicall would revert — a bad
        // signature, a spent nonce, a closed window, a reverting call — and the
        // request is refused without a fee having been paid.
        let estimate = execution
            .estimate_fee()
            .await
            .map_err(|e| SponsorError::Declined(format!("would revert, so not submitted: {e}")))?;

        let l1_gas = pad(u128::from(estimate.l1_gas_consumed));
        let l2_gas = pad(u128::from(estimate.l2_gas_consumed));
        let l1_data_gas = pad(u128::from(estimate.l1_data_gas_consumed));
        let l1_gas_price = pad(estimate.l1_gas_price);
        let l2_gas_price = pad(estimate.l2_gas_price);
        let l1_data_gas_price = pad(estimate.l1_data_gas_price);

        // The number the ceiling is checked against is the *bound*, not the estimate:
        // the bound is what the sponsor authorises the sequencer to take.
        let bound = worst_case_fee(
            l1_gas,
            l1_gas_price,
            l2_gas,
            l2_gas_price,
            l1_data_gas,
            l1_data_gas_price,
        );
        if bound > self.max_fee_fri {
            warn!(
                bound,
                estimated = estimate.overall_fee,
                ceiling = self.max_fee_fri,
                "refusing a request above the per-transaction fee ceiling"
            );
            return Err(SponsorError::Declined(format!(
                "fee bound {bound} Fri exceeds the ceiling of {} Fri \
                 (estimated {} Fri before padding)",
                self.max_fee_fri, estimate.overall_fee
            )));
        }

        let gas_u64 = |name: &'static str, v: u128| -> Result<u64, SponsorError> {
            u64::try_from(v)
                .map_err(|_| SponsorError::Declined(format!("{name} bound {v} does not fit a u64")))
        };

        let sent = execution
            .l1_gas(gas_u64("l1_gas", l1_gas)?)
            .l1_gas_price(l1_gas_price)
            .l2_gas(gas_u64("l2_gas", l2_gas)?)
            .l2_gas_price(l2_gas_price)
            .l1_data_gas(gas_u64("l1_data_gas", l1_data_gas)?)
            .l1_data_gas_price(l1_data_gas_price)
            .send()
            .await
            // Reported, never retried here: a blind retry is how a sponsor pays twice
            // for one request.
            .map_err(|e| SponsorError::Chain(format!("submission rejected: {e}")))?;

        info!(
            tx = %sent.transaction_hash.to_fixed_hex_string(),
            nonce = %nonce,
            bound,
            estimated = estimate.overall_fee,
            "submitted a sponsored transaction"
        );
        Ok(sent.transaction_hash)
    }
}

/// The most a transaction with these bounds can cost, in Fri.
///
/// Saturating: an overflow must read as "unaffordable" and be refused, never wrap to
/// a small number that passes the ceiling.
pub fn worst_case_fee(
    l1_gas: u128,
    l1_gas_price: u128,
    l2_gas: u128,
    l2_gas_price: u128,
    l1_data_gas: u128,
    l1_data_gas_price: u128,
) -> u128 {
    l1_gas
        .saturating_mul(l1_gas_price)
        .saturating_add(l2_gas.saturating_mul(l2_gas_price))
        .saturating_add(l1_data_gas.saturating_mul(l1_data_gas_price))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_is_one_and_a_half_times() {
        assert_eq!(pad(100), 150);
        assert_eq!(pad(1), 1, "integer division truncates, never inflates");
        assert_eq!(pad(0), 0);
    }

    #[test]
    fn padding_saturates_rather_than_wrapping() {
        // The multiply saturates before the divide, so an absurd estimate yields an
        // absurd bound that the ceiling then refuses. The property that matters is
        // that it stays huge rather than wrapping to something small enough to pass.
        let padded = pad(u128::MAX);
        assert_eq!(padded, u128::MAX / BOUND_DENOMINATOR);
        assert!(padded > DEFAULT_MAX_FEE_FRI);
    }

    #[test]
    fn the_bound_is_the_padded_estimate_squared_in_effect() {
        // 1.5x gas and 1.5x price is 2.25x fee, which is the number the ceiling must
        // be set against — not the raw estimate.
        let (gas, price) = (1_000u128, 1_000u128);
        let raw = gas * price;
        let bound = worst_case_fee(pad(gas), pad(price), 0, 0, 0, 0);
        assert_eq!(bound, raw * 9 / 4);
    }

    #[test]
    fn the_worst_case_sums_all_three_resources() {
        assert_eq!(worst_case_fee(2, 3, 5, 7, 11, 13), 6 + 35 + 143);
    }

    #[test]
    fn an_overflowing_bound_reads_as_unaffordable() {
        // Must saturate high so the ceiling refuses it, not wrap low so it passes.
        assert_eq!(worst_case_fee(u128::MAX, 2, 0, 0, 0, 0), u128::MAX);
        assert_eq!(
            worst_case_fee(u128::MAX, 1, u128::MAX, 1, 0, 0),
            u128::MAX,
            "the sum must saturate too"
        );
    }

    #[test]
    fn the_default_ceiling_covers_a_measured_deploy_and_execute() {
        // ~0.92 STRK estimated on mainnet, 2.25x padded, so the default must clear
        // ~2.1 STRK or every real request would be refused.
        const MEASURED_ESTIMATE_FRI: u128 = 923_108_179_789_507_228;
        let bound = MEASURED_ESTIMATE_FRI * 9 / 4;
        assert!(
            bound < DEFAULT_MAX_FEE_FRI,
            "default ceiling {DEFAULT_MAX_FEE_FRI} would refuse a legitimate {bound}"
        );
        // And it must still be a real limit, not effectively unbounded.
        assert!(DEFAULT_MAX_FEE_FRI < bound * 10);
    }

    #[test]
    fn the_tip_is_zero_so_the_ceiling_means_something() {
        // A tip is fee outside the gas bounds; any non-zero value makes the ceiling
        // an underestimate of what a transaction can take.
        assert_eq!(TIP, 0);
    }
}
