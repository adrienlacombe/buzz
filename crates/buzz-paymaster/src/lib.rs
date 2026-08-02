//! Sponsors Starknet transactions for Nostr-key-controlled accounts.
//!
//! A `NostrAccount` pays ~0.78 STRK of BIP-340 verification before a transaction
//! does anything useful, so a freshly created account holds nothing and can do
//! nothing. This crate is the sponsor: it pays deployment and execution fees on a
//! user's behalf, from an account it controls.
//!
//! # Why this is a separate crate
//!
//! It holds spending authority. The relay does not, and should not — a compromise
//! of the relay process should not be a compromise of a funded wallet. Keeping the
//! sponsor a separate deployable keeps that boundary real rather than notional.
//!
//! # What it can and cannot do
//!
//! The sponsor decides *whether* to pay, never what executes. A user's calls are
//! authorised by their own BIP-340 signature over a SNIP-9 payload
//! ([`buzz_core::outside_execution`]); the sponsor cannot alter the calls and
//! cannot forge the signature. The worst a malicious sponsor can do is refuse
//! service, or submit a payload the user already signed — which is why the payload
//! carries a nonce and an expiry window.
//!
//! # Deployment is lazy, and has two triggers
//!
//! Account addresses are counterfactual: derivable from the pubkey alone, and able
//! to receive funds before the account exists. So nothing is deployed at signup, and
//! the sponsor never pays for the majority of users who join and never transact.
//!
//! Deployment then happens at whichever of these comes first:
//!
//! 1. **A first sponsored transaction** ([`buzz_core::kind::KIND_SPONSOR_REQUEST`]).
//!    The deployment rides along in the same atomic multicall, so a user who wants
//!    to *do* something never sees a separate step.
//! 2. **The account being funded**
//!    ([`buzz_core::kind::KIND_SPONSOR_DEPLOY_REQUEST`]). A user who has sent STRK to
//!    their address and expects it to become real has nothing to sign and no
//!    transaction to make, and would otherwise have to spend ~0.92 STRK deploying it
//!    themselves.
//!
//! The second is *cheaper* than the first: its multicall is the UDC deploy alone,
//! with no `execute_from_outside_v2`, so it skips the ~0.78 STRK of on-chain BIP-340
//! verification.
//!
//! ## Why the funding trigger needs a floor
//!
//! Addresses derive from Nostr pubkeys, and pubkeys are public on the relay — so
//! anyone can compute every member's address and send dust to it. The member's own
//! signed event is what asks for the deployment, but a client that watches its
//! balance would ask automatically, so dust alone could set it off. A minimum
//! balance ([`config::DEFAULT_MIN_DEPLOY_BALANCE`], read from the chain and never
//! from the request) makes dusting the membership cost the attacker roughly what it
//! costs the sponsor.
//!
//! # Layering
//!
//! Everything here is pure decision logic over a [`Chain`] abstraction. Nothing in
//! this crate opens a socket, and nothing signs with the sponsor's key — that lives
//! behind the trait, so the policy and call construction are testable without a
//! node and without funds.

use buzz_core::outside_execution::{OutsideCall, OutsideExecution};
use buzz_core::sponsorship::SponsorRequest;
use starknet_core::types::{Call, Felt};
use starknet_core::utils::get_selector_from_name;

pub mod config;
pub mod handler;
pub mod rpc;
pub mod service;
pub mod submitter;
pub mod ws;

/// Errors from sponsorship.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SponsorError {
    /// The chain could not be reached, or answered unusably.
    #[error("chain query failed: {0}")]
    Chain(String),
    /// Policy declined to sponsor this request.
    #[error("declined: {0}")]
    Declined(String),
    /// Configuration is missing or unusable.
    #[error("misconfigured: {0}")]
    Config(String),
}

/// The chain operations sponsorship needs.
///
/// A trait so the logic above it is testable without a node. The real
/// implementation is deliberately thin — if a bug can live in it, it should have
/// been pushed up into the tested logic instead.
pub trait Chain {
    /// Whether a contract exists at `address`.
    ///
    /// Used to decide deployment. Implementations should treat "class hash not
    /// found" as `false` rather than an error, and anything else as an error: a
    /// transport failure must not be read as "not deployed", or the sponsor would
    /// pay to deploy an account that already exists.
    fn is_deployed(
        &self,
        address: Felt,
    ) -> impl std::future::Future<Output = Result<bool, SponsorError>> + Send;

    /// The `token` balance held at `address`, in the token's smallest unit.
    ///
    /// Used to decide whether an account has been *funded*, which is what
    /// authorises sponsoring its deployment. An address that does not exist yet
    /// still has a balance — that is the whole point of a counterfactual address —
    /// so this must answer for undeployed addresses too rather than erroring.
    ///
    /// Returned as `u128` rather than a full `u256`: STRK's supply cannot approach
    /// 2^128 in its smallest unit, and the value is only ever compared against a
    /// small threshold. An implementation that finds a non-zero high limb should
    /// saturate rather than truncate — reading a huge balance as a small one would
    /// refuse to deploy a genuinely funded account.
    fn balance_of(
        &self,
        token: Felt,
        address: Felt,
    ) -> impl std::future::Future<Output = Result<u128, SponsorError>> + Send;
}

/// The STRK fee token on Starknet mainnet.
///
/// Provenance, stated so it can be re-checked rather than trusted: verified live
/// against `mainnet.nodes.starknet.org`, where `symbol()` returns `STRK` — see
/// `rpc::tests::the_documented_fee_token_is_strk_on_mainnet`, an `#[ignore]`d test
/// that makes the check reproducible instead of a claim in a comment. Run it with:
///
/// ```text
/// cargo test -p buzz-paymaster -- --ignored the_documented_fee_token
/// ```
///
/// A wrong token address here reads every balance as zero, so every deployment
/// request would be refused as unfunded — a visible failure rather than a costly
/// one, but a confusing one to debug.
pub const STRK_MAINNET: Felt =
    Felt::from_hex_unchecked("0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d");

/// The chain the sponsor operates on, and what it deploys there.
///
/// Bundled rather than passed as three loose `Felt`s: a transposed `class_hash` and
/// `udc` would type-check and then deploy nothing while still charging a fee, and
/// there is no later step that would catch it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainConfig {
    /// Class accounts are deployed from.
    pub class_hash: Felt,
    /// Universal Deployer address.
    pub udc: Felt,
    /// Chain id as a felt, e.g. `SN_MAIN` encoded as a Cairo short string.
    ///
    /// Read from the node rather than configured, so it cannot disagree with the
    /// chain the sponsor is actually submitting to.
    pub chain_id: Felt,
    /// Token whose balance decides whether an account counts as funded.
    ///
    /// [`STRK_MAINNET`] on mainnet. A parameter rather than a constant because the
    /// address differs per network, and a wrong one reads every balance as zero —
    /// refusing every deployment request as unfunded.
    pub fee_token: Felt,
}

impl ChainConfig {
    /// Whether a request's `chain_id` short string names this chain.
    ///
    /// # Why this is checked before submitting
    ///
    /// The user's signature covers a SNIP-12 hash that embeds the chain id, so a
    /// request signed for another network cannot verify here — it reverts, and the
    /// sponsor pays for the revert. With no per-member quota, publishing
    /// wrong-chain requests in a loop would otherwise be a way to drain the sponsor
    /// using nothing but valid membership.
    pub fn chain_id_matches(&self, short_string: &str) -> bool {
        starknet_core::utils::cairo_short_string_to_felt(short_string)
            .map(|f| f == self.chain_id)
            .unwrap_or(false)
    }
}

/// What the sponsor must do to service a request.
///
/// Returned rather than executed so the decision can be asserted in tests and
/// logged before anything is paid for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SponsorPlan {
    /// Deploy the account, then run the calls. First use by this pubkey.
    DeployThenExecute {
        /// The account's counterfactual address.
        address: Felt,
    },
    /// The account exists; only the calls need submitting.
    ExecuteOnly {
        /// The account's address.
        address: Felt,
    },
}

impl SponsorPlan {
    /// The account address this plan targets.
    pub fn address(&self) -> Felt {
        match self {
            SponsorPlan::DeployThenExecute { address } | SponsorPlan::ExecuteOnly { address } => {
                *address
            }
        }
    }

    /// Whether this plan includes a deployment.
    pub fn deploys(&self) -> bool {
        matches!(self, SponsorPlan::DeployThenExecute { .. })
    }
}

/// Decides what servicing a request requires.
///
/// Derives the address from the pubkey and the configured class hash, then asks the
/// chain whether it exists. The address is *always* derived rather than accepted
/// from the caller: taking a caller-supplied address would let anyone direct a
/// sponsored deployment at an arbitrary contract.
pub async fn plan_for(
    chain: &impl Chain,
    class_hash: Felt,
    nostr_pubkey: &str,
) -> Result<SponsorPlan, SponsorError> {
    let address = buzz_core::starknet_account::account_address(class_hash, nostr_pubkey)
        .map_err(|e| SponsorError::Config(format!("cannot derive address: {e}")))?;
    if chain.is_deployed(address).await? {
        Ok(SponsorPlan::ExecuteOnly { address })
    } else {
        Ok(SponsorPlan::DeployThenExecute { address })
    }
}

/// Universal Deployer Contract entry point, `deployContract`.
///
/// Deployment goes through the UDC rather than a `DEPLOY_ACCOUNT` transaction
/// because `DEPLOY_ACCOUNT` is paid by the account being deployed — which is empty,
/// which is the whole problem. A UDC call is an ordinary `INVOKE` from the
/// sponsor's funded account.
pub const UDC_DEPLOY_CONTRACT_SELECTOR: &str = "deployContract";

/// SNIP-9 sponsored-execution entry point on the user's account.
pub const EXECUTE_FROM_OUTSIDE_SELECTOR: &str = "execute_from_outside_v2";

/// The Universal Deployer on Starknet mainnet.
///
/// Provided as a default rather than a hardcoded value: it is a parameter
/// everywhere below, because a wrong UDC address silently deploys nothing while
/// still charging the sponsor.
///
/// Provenance, stated so it can be re-checked rather than trusted: this is the
/// address `avnu-labs/paymaster` uses (`paymaster-starknet/src/constants.rs`), and
/// `starknet_getClassHashAt` on mainnet confirms a contract is deployed there.
/// **That confirms existence, not semantics** — verify it really is the UDC, with
/// the `deployContract` signature assumed here, before pointing funds at it.
pub const UDC_MAINNET: Felt =
    Felt::from_hex_unchecked("0x041a78e741e5af2fec34b695679bc6891742439f7afb8484ecd7766661ad02bf");

/// Builds the UDC calldata that deploys a user's account at its derived address.
///
/// # The invariant that matters
///
/// `deploy_from_zero` must be **true**. Our address derivation uses
/// `deployer_address = 0` (as `DEPLOY_ACCOUNT` does), and only `deploy_from_zero`
/// makes the `deploy` syscall use zero too. With it false, the UDC's own address is
/// mixed into the hash and the account lands somewhere nobody derived — reachable
/// by no client, holding whatever was sent to the address the user was shown.
///
/// The literal `1` below is that flag. It is the single most consequential value in
/// this crate.
pub fn udc_deploy_calldata(
    class_hash: Felt,
    nostr_pubkey: &str,
) -> Result<Vec<Felt>, SponsorError> {
    let ctor = buzz_core::starknet_account::constructor_calldata(nostr_pubkey)
        .map_err(|e| SponsorError::Config(format!("cannot build constructor calldata: {e}")))?;
    let mut out = vec![
        class_hash,
        buzz_core::starknet_account::DEPLOY_SALT,
        Felt::ONE, // deploy_from_zero — see the note above
        Felt::from(ctor.len()),
    ];
    out.extend_from_slice(&ctor);
    Ok(out)
}

/// The calls a sponsor submits as **one** transaction to service a request.
///
/// Starknet executes a multicall atomically, so returning both calls together is
/// what makes deploy-then-execute all-or-nothing: if the deployment reverts, the
/// user's calls never run, and the sponsor is not left having paid to create an
/// account whose first action failed separately.
///
/// Order is load-bearing. The deployment must come first, or the execute call
/// targets an address with no contract at it.
pub fn build_atomic_calls(
    plan: &SponsorPlan,
    config: &ChainConfig,
    nostr_pubkey: &str,
    execution: &OutsideExecution,
    signature: &[Felt; 4],
) -> Result<Vec<Call>, SponsorError> {
    let ChainConfig {
        class_hash, udc, ..
    } = *config;
    // Re-derive rather than trust the plan: this function is the last point before
    // money is spent, and a plan carrying an address that does not match the pubkey
    // would aim a sponsored call at a contract the user does not control.
    let derived = buzz_core::starknet_account::account_address(class_hash, nostr_pubkey)
        .map_err(|e| SponsorError::Config(format!("cannot derive address: {e}")))?;
    if plan.address() != derived {
        return Err(SponsorError::Declined(format!(
            "plan address {:#x} does not match the address derived for this pubkey ({:#x})",
            plan.address(),
            derived
        )));
    }

    let mut calls = Vec::with_capacity(2);
    if plan.deploys() {
        calls.push(Call {
            to: udc,
            selector: get_selector_from_name(UDC_DEPLOY_CONTRACT_SELECTOR)
                .map_err(|e| SponsorError::Config(format!("bad UDC selector: {e}")))?,
            calldata: udc_deploy_calldata(class_hash, nostr_pubkey)?,
        });
    }
    calls.push(Call {
        // The user's own account. The signature was computed over a hash binding
        // this address, so sending elsewhere cannot succeed — it would be rejected
        // rather than misapplied, but it would still cost the sponsor a fee.
        to: derived,
        selector: get_selector_from_name(EXECUTE_FROM_OUTSIDE_SELECTOR)
            .map_err(|e| SponsorError::Config(format!("bad execute selector: {e}")))?,
        calldata: buzz_core::outside_execution::execute_from_outside_calldata(execution, signature),
    });
    Ok(calls)
}

/// Builds the one call that deploys a member's account and nothing else.
///
/// This is the funded-wallet trigger: no SNIP-9 payload, no
/// `execute_from_outside_v2`, so none of the ~0.78 STRK of on-chain BIP-340
/// verification. Deploying this way costs the sponsor strictly less than deploying
/// as a side effect of a first sponsored transaction.
///
/// Authorisation is the event signature the relay already verified, and the address
/// is derived from the author — so the only account a member can ask to have
/// deployed is their own, owned by their own pubkey.
pub fn build_deploy_only_calls(
    config: &ChainConfig,
    nostr_pubkey: &str,
) -> Result<Vec<Call>, SponsorError> {
    Ok(vec![Call {
        to: config.udc,
        selector: get_selector_from_name(UDC_DEPLOY_CONTRACT_SELECTOR)
            .map_err(|e| SponsorError::Config(format!("bad UDC selector: {e}")))?,
        calldata: udc_deploy_calldata(config.class_hash, nostr_pubkey)?,
    }])
}

/// Decides whether to sponsor deploying a funded account, and builds the call.
///
/// Refuses, in order of how little it costs to find out:
/// 1. Wrong chain — no RPC at all.
/// 2. Already deployed — one RPC, and paying again would be pure waste.
/// 3. Not funded to `min_balance` — one more RPC.
///
/// The balance is read from the chain, never taken from the request, which is what
/// makes "funded" mean funded rather than claimed.
pub async fn service_deploy_request(
    chain: &impl Chain,
    config: &ChainConfig,
    author_pubkey: &str,
    request: &buzz_core::sponsorship::DeployRequest,
    d_tag: &str,
    min_balance: u128,
) -> Result<Vec<Call>, SponsorError> {
    request
        .validate()
        .map_err(|e| SponsorError::Declined(e.to_string()))?;
    if !config.chain_id_matches(&request.chain_id) {
        return Err(SponsorError::Declined(format!(
            "request is for chain {:?}, but this sponsor serves {:#x}",
            request.chain_id, config.chain_id
        )));
    }
    if !request.chain_id_matches_d_tag(d_tag) {
        // The d tag is the replaceable key. Letting it differ from the chain id would
        // give one member several deployment slots on one chain, and the dedupe set
        // is keyed on it.
        return Err(SponsorError::Declined(format!(
            "chain_id {} does not match the event d tag {d_tag}",
            request.chain_id
        )));
    }

    let plan = plan_for(chain, config.class_hash, author_pubkey).await?;
    if !plan.deploys() {
        // Not an error: the member asked for something that has already happened.
        return Err(SponsorError::Declined(
            "account is already deployed; nothing to do".into(),
        ));
    }

    // The funding check, and the reason this trigger is safe to expose. Anyone can
    // send dust to any member's address — addresses are derivable from public
    // pubkeys — so without a floor, dusting the membership would turn into a
    // sponsored deployment for each of them.
    let balance = chain.balance_of(config.fee_token, plan.address()).await?;
    if balance < min_balance {
        return Err(SponsorError::Declined(format!(
            "account holds {balance} of the fee token, below the {min_balance} \
             required to sponsor a deployment"
        )));
    }

    build_deploy_only_calls(config, author_pubkey)
}

/// Turns a member's published request into the calls that service it.
///
/// This is the whole service path: parse, refuse anything that cannot succeed,
/// derive the account, ask whether it exists, and produce the multicall. The only
/// thing left is submission, which needs a funded signer.
///
/// The user's pubkey comes from the **event author**, never the payload. That is
/// what stops one member requesting sponsorship into another member's account: the
/// relay has already verified the event signature, so the author is attested, and
/// the account address is derived from it rather than supplied.
pub async fn service_request(
    chain: &impl Chain,
    config: &ChainConfig,
    author_pubkey: &str,
    request: &SponsorRequest,
    d_tag: &str,
) -> Result<Vec<Call>, SponsorError> {
    request
        .validate()
        .map_err(|e| SponsorError::Declined(e.to_string()))?;
    if !config.chain_id_matches(&request.chain_id) {
        // The signature embeds the chain id, so this could only ever revert — at the
        // sponsor's expense. Cheapest possible refusal: no chain query, no fee.
        return Err(SponsorError::Declined(format!(
            "request is for chain {:?}, but this sponsor serves {:#x}",
            request.chain_id, config.chain_id
        )));
    }
    if !request.nonce_matches_d_tag(d_tag) {
        // Replacement is keyed on the d tag while on-chain replay protection is
        // keyed on the nonce. Allowing them to differ would let two distinct
        // requests share one replaceable slot, so a resend could smuggle in
        // different calls under an already-seen address.
        return Err(SponsorError::Declined(format!(
            "nonce {} does not match the event d tag {d_tag}",
            request.nonce
        )));
    }

    let felt = |field: &'static str, v: &str| -> Result<Felt, SponsorError> {
        Felt::from_hex(v).map_err(|_| SponsorError::Declined(format!("invalid {field}: {v:?}")))
    };

    let mut calls = Vec::with_capacity(request.calls.len());
    for c in &request.calls {
        let mut calldata = Vec::with_capacity(c.calldata.len());
        for d in &c.calldata {
            calldata.push(felt("calldata", d)?);
        }
        calls.push(OutsideCall {
            to: felt("call.to", &c.to)?,
            selector: felt("call.selector", &c.selector)?,
            calldata,
        });
    }
    let execution = OutsideExecution {
        caller: felt("caller", &request.caller)
            .unwrap_or_else(|_| buzz_core::outside_execution::any_caller()),
        nonce: felt("nonce", &request.nonce)?,
        execute_after: request.execute_after,
        execute_before: request.execute_before,
        calls,
    };
    let mut signature = [Felt::ZERO; 4];
    for (i, s) in request.signature.iter().enumerate() {
        signature[i] = felt("signature", s)?;
    }

    let plan = plan_for(chain, config.class_hash, author_pubkey).await?;
    build_atomic_calls(&plan, config, author_pubkey, &execution, &signature)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BIP-340 test vector 0's public key — a published, deliberately non-secret
    /// value, so no key was generated for these tests.
    const PUBKEY: &str = "f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9";
    /// The SNIP-9 class declared on mainnet 2026-08-02, declare tx
    /// `0x05328994e14ed537c34f3a19a79e4bad71d3be560fe47da4067dd7014c4399fc`. Used
    /// here only to make the test addresses realistic; production reads it from
    /// configuration, since a class hash changes with any contract edit.
    const CLASS_HASH: &str = "0x0414f62ea1ed35f8c7bd3b794d94efc95e01bccf04e0f47211fc198f7f56f537";

    /// One STRK, the default funding floor.
    const ONE_STRK: u128 = 1_000_000_000_000_000_000;

    struct FakeChain {
        deployed: bool,
        balance: u128,
    }
    impl FakeChain {
        /// Funded well above the floor, for the tests where funding is not the
        /// subject.
        fn funded(deployed: bool) -> Self {
            Self {
                deployed,
                balance: ONE_STRK * 5,
            }
        }
    }
    impl Chain for FakeChain {
        async fn is_deployed(&self, _address: Felt) -> Result<bool, SponsorError> {
            Ok(self.deployed)
        }
        async fn balance_of(&self, _t: Felt, _a: Felt) -> Result<u128, SponsorError> {
            Ok(self.balance)
        }
    }
    struct BrokenChain;
    impl Chain for BrokenChain {
        async fn is_deployed(&self, _address: Felt) -> Result<bool, SponsorError> {
            Err(SponsorError::Chain("timeout".into()))
        }
        async fn balance_of(&self, _t: Felt, _a: Felt) -> Result<u128, SponsorError> {
            Err(SponsorError::Chain("timeout".into()))
        }
    }

    fn class_hash() -> Felt {
        Felt::from_hex_unchecked(CLASS_HASH)
    }

    /// Mainnet, as the fixtures' `SN_MAIN` requests expect.
    fn cfg() -> ChainConfig {
        ChainConfig {
            class_hash: class_hash(),
            udc: UDC_MAINNET,
            chain_id: starknet_core::utils::cairo_short_string_to_felt("SN_MAIN").unwrap(),
            fee_token: STRK_MAINNET,
        }
    }

    #[tokio::test]
    async fn undeployed_accounts_are_planned_for_deployment() {
        let plan = plan_for(&FakeChain::funded(false), class_hash(), PUBKEY)
            .await
            .unwrap();
        assert!(plan.deploys());
    }

    #[tokio::test]
    async fn deployed_accounts_skip_deployment() {
        let plan = plan_for(&FakeChain::funded(true), class_hash(), PUBKEY)
            .await
            .unwrap();
        assert!(!plan.deploys(), "paying to redeploy would be pure waste");
    }

    #[tokio::test]
    async fn a_chain_error_is_never_read_as_not_deployed() {
        // The dangerous misreading: treat a timeout as "does not exist" and pay to
        // deploy over a live account.
        assert!(matches!(
            plan_for(&BrokenChain, class_hash(), PUBKEY).await,
            Err(SponsorError::Chain(_))
        ));
    }

    #[tokio::test]
    async fn the_planned_address_is_the_one_clients_derive() {
        // If these ever diverge, the sponsor funds an address no client can reach.
        let plan = plan_for(&FakeChain::funded(false), class_hash(), PUBKEY)
            .await
            .unwrap();
        let expected = buzz_core::starknet_account::account_address(class_hash(), PUBKEY).unwrap();
        assert_eq!(plan.address(), expected);
    }

    #[tokio::test]
    async fn a_bad_pubkey_is_a_config_error_not_a_panic() {
        assert!(matches!(
            plan_for(&FakeChain::funded(false), class_hash(), "nonsense").await,
            Err(SponsorError::Config(_))
        ));
    }

    #[test]
    fn udc_calldata_sets_deploy_from_zero() {
        // The single most consequential value in this crate: with this false, the
        // account lands at an address nobody derived.
        let data = udc_deploy_calldata(class_hash(), PUBKEY).unwrap();
        assert_eq!(data[0], class_hash(), "class hash");
        assert_eq!(
            data[1],
            buzz_core::starknet_account::DEPLOY_SALT,
            "salt must match the derivation"
        );
        assert_eq!(data[2], Felt::ONE, "deploy_from_zero MUST be true");
    }

    #[test]
    fn udc_calldata_carries_the_pubkey_as_constructor_args() {
        let data = udc_deploy_calldata(class_hash(), PUBKEY).unwrap();
        let ctor = buzz_core::starknet_account::constructor_calldata(PUBKEY).unwrap();
        assert_eq!(data[3], Felt::from(ctor.len()), "calldata length prefix");
        assert_eq!(&data[4..], &ctor[..]);
        assert_eq!(ctor.len(), 2, "a u256 pubkey is two felts");
    }

    #[test]
    fn udc_deployment_reproduces_the_derived_address() {
        // Ties the two derivations together: the constructor calldata the UDC gets
        // is the same calldata the address was derived from, and the salt matches.
        // Given deploy_from_zero, the syscall's address formula is then identical
        // to account_address's.
        let data = udc_deploy_calldata(class_hash(), PUBKEY).unwrap();
        let ctor = buzz_core::starknet_account::constructor_calldata(PUBKEY).unwrap();
        let derived = buzz_core::starknet_account::account_address(class_hash(), PUBKEY).unwrap();
        let recomputed = starknet_core::utils::get_contract_address(
            data[1], // salt from the calldata we are about to send
            data[0], // class hash from the same
            &ctor,
            Felt::ZERO, // deployer zero, which deploy_from_zero=1 selects
        );
        assert_eq!(recomputed, derived);
    }

    #[tokio::test]
    async fn different_pubkeys_get_different_accounts() {
        let other = "dff1d77f2a671c5f36183726db2341be58feae1da2deced843240f7b502ba659";
        let a = plan_for(&FakeChain::funded(false), class_hash(), PUBKEY)
            .await
            .unwrap();
        let b = plan_for(&FakeChain::funded(false), class_hash(), other)
            .await
            .unwrap();
        assert_ne!(a.address(), b.address());
    }

    fn an_execution() -> OutsideExecution {
        OutsideExecution {
            caller: buzz_core::outside_execution::any_caller(),
            nonce: Felt::from(7_u32),
            execute_after: 100,
            execute_before: 200,
            calls: vec![buzz_core::outside_execution::OutsideCall {
                to: Felt::from(0xabc_u32),
                selector: Felt::from(0xdef_u32),
                calldata: vec![Felt::ONE],
            }],
        }
    }

    fn a_signature() -> [Felt; 4] {
        [Felt::ONE, Felt::TWO, Felt::THREE, Felt::from(4_u32)]
    }

    async fn calls_for(deployed: bool) -> Vec<Call> {
        let plan = plan_for(&FakeChain::funded(deployed), class_hash(), PUBKEY)
            .await
            .unwrap();
        build_atomic_calls(&plan, &cfg(), PUBKEY, &an_execution(), &a_signature()).unwrap()
    }

    #[tokio::test]
    async fn an_existing_account_gets_one_call() {
        let calls = calls_for(true).await;
        assert_eq!(calls.len(), 1, "no deployment should be paid for");
        assert_eq!(
            calls[0].to,
            buzz_core::starknet_account::account_address(class_hash(), PUBKEY).unwrap()
        );
        assert_eq!(
            calls[0].selector,
            get_selector_from_name(EXECUTE_FROM_OUTSIDE_SELECTOR).unwrap()
        );
    }

    #[tokio::test]
    async fn a_new_account_gets_deploy_then_execute_in_that_order() {
        let calls = calls_for(false).await;
        assert_eq!(calls.len(), 2);
        // Order is the whole point: reversed, the execute call would target an
        // address with no contract at it and the transaction would revert after the
        // sponsor had already committed to the fee.
        assert_eq!(calls[0].to, UDC_MAINNET, "deploy must be first");
        assert_eq!(
            calls[0].selector,
            get_selector_from_name(UDC_DEPLOY_CONTRACT_SELECTOR).unwrap()
        );
        assert_eq!(
            calls[1].to,
            buzz_core::starknet_account::account_address(class_hash(), PUBKEY).unwrap(),
            "execute must target the account the deploy just created"
        );
    }

    #[tokio::test]
    async fn the_deploy_call_creates_the_account_the_execute_call_targets() {
        // The two calls must agree, or the multicall deploys one account and calls
        // another. Recompute the deployed address from the deploy calldata itself.
        let calls = calls_for(false).await;
        let d = &calls[0].calldata;
        let recomputed =
            starknet_core::utils::get_contract_address(d[1], d[0], &d[4..], Felt::ZERO);
        assert_eq!(recomputed, calls[1].to);
    }

    #[tokio::test]
    async fn the_signed_payload_is_passed_through_unaltered() {
        // The sponsor must not be able to change what executes. Whatever the user
        // signed is what goes on the wire.
        let exec = an_execution();
        let sig = a_signature();
        let calls = calls_for(true).await;
        assert_eq!(
            calls[0].calldata,
            buzz_core::outside_execution::execute_from_outside_calldata(&exec, &sig)
        );
    }

    #[test]
    fn a_plan_whose_address_does_not_match_the_pubkey_is_declined() {
        // Defence in depth against a plan built elsewhere: this is the last point
        // before money is spent.
        let forged = SponsorPlan::ExecuteOnly {
            address: Felt::from(0xdead_u32),
        };
        assert!(matches!(
            build_atomic_calls(&forged, &cfg(), PUBKEY, &an_execution(), &a_signature()),
            Err(SponsorError::Declined(_))
        ));
    }

    #[test]
    fn the_udc_default_is_the_documented_address() {
        // Pinned so a typo cannot silently redirect deployments. Provenance and the
        // limits of that verification are on the constant itself.
        assert_eq!(
            UDC_MAINNET,
            Felt::from_hex_unchecked(
                "0x041a78e741e5af2fec34b695679bc6891742439f7afb8484ecd7766661ad02bf"
            )
        );
    }

    fn a_request() -> SponsorRequest {
        SponsorRequest {
            chain_id: "SN_MAIN".into(),
            caller: "0x0".into(),
            nonce: "0x2a".into(),
            execute_after: 1_000,
            execute_before: 2_000,
            calls: vec![buzz_core::sponsorship::SponsorCall {
                to: "0x1234".into(),
                selector: "0x5678".into(),
                calldata: vec!["0x1".into()],
            }],
            signature: vec!["0x1".into(), "0x2".into(), "0x3".into(), "0x4".into()],
        }
    }

    #[tokio::test]
    async fn a_request_from_a_new_member_deploys_and_executes() {
        let calls = service_request(
            &FakeChain::funded(false),
            &cfg(),
            PUBKEY,
            &a_request(),
            "0x2a",
        )
        .await
        .unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].to, UDC_MAINNET);
    }

    #[tokio::test]
    async fn a_request_from_an_existing_member_only_executes() {
        let calls = service_request(
            &FakeChain::funded(true),
            &cfg(),
            PUBKEY,
            &a_request(),
            "0x2a",
        )
        .await
        .unwrap();
        assert_eq!(calls.len(), 1);
    }

    #[tokio::test]
    async fn the_account_is_derived_from_the_event_author_not_the_payload() {
        // The payload carries no address at all, by design: the relay attests the
        // author, so deriving from the author is what stops one member requesting
        // sponsorship into another member's account.
        let calls = service_request(
            &FakeChain::funded(true),
            &cfg(),
            PUBKEY,
            &a_request(),
            "0x2a",
        )
        .await
        .unwrap();
        assert_eq!(
            calls[0].to,
            buzz_core::starknet_account::account_address(class_hash(), PUBKEY).unwrap()
        );
    }

    #[tokio::test]
    async fn a_nonce_that_disagrees_with_the_d_tag_is_declined() {
        let err = service_request(
            &FakeChain::funded(true),
            &cfg(),
            PUBKEY,
            &a_request(),
            "0xdifferent",
        )
        .await;
        assert!(matches!(err, Err(SponsorError::Declined(_))));
    }

    #[tokio::test]
    async fn an_invalid_request_is_declined_before_any_chain_query() {
        // BrokenChain errors on any query, so reaching it would fail with Chain
        // rather than Declined. Getting Declined proves validation ran first —
        // which is the point: a bad request must not cost a round trip, let alone a
        // fee.
        let mut r = a_request();
        r.calls.clear();
        assert!(matches!(
            service_request(&BrokenChain, &cfg(), PUBKEY, &r, "0x2a").await,
            Err(SponsorError::Declined(_))
        ));
    }

    #[tokio::test]
    async fn a_request_for_another_chain_is_declined_before_any_chain_query() {
        // The signature embeds the chain id, so this could only revert — at the
        // sponsor's expense. With no per-member quota, a loop of these would
        // otherwise drain the sponsor using nothing but valid membership.
        // BrokenChain proves no query was made.
        let mut r = a_request();
        r.chain_id = "SN_SEPOLIA".into();
        assert!(matches!(
            service_request(&BrokenChain, &cfg(), PUBKEY, &r, "0x2a").await,
            Err(SponsorError::Declined(_))
        ));
    }

    #[test]
    fn chain_id_matching_encodes_the_short_string() {
        assert!(cfg().chain_id_matches("SN_MAIN"));
        assert!(!cfg().chain_id_matches("SN_SEPOLIA"));
        // Anything unencodable is a mismatch, never a match by accident.
        assert!(!cfg().chain_id_matches(&"A".repeat(32)));
        assert!(!cfg().chain_id_matches(""));
    }

    #[tokio::test]
    async fn unparseable_felts_are_declined_not_panics() {
        let mut r = a_request();
        r.calls[0].to = "not-a-felt".into();
        assert!(matches!(
            service_request(&FakeChain::funded(true), &cfg(), PUBKEY, &r, "0x2a").await,
            Err(SponsorError::Declined(_))
        ));
    }
}
