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
//! # Deployment is lazy
//!
//! Account addresses are counterfactual: derivable from the pubkey alone, and able
//! to receive funds before the account exists. So nothing is deployed at signup.
//! An account is deployed on first use, which means the sponsor never pays for the
//! majority of users who join and never transact.
//!
//! # Layering
//!
//! Everything here is pure decision logic over a [`Chain`] abstraction. Nothing in
//! this crate opens a socket, and nothing signs with the sponsor's key — that lives
//! behind the trait, so the policy and call construction are testable without a
//! node and without funds.

use buzz_core::outside_execution::OutsideExecution;
use starknet_core::types::{Call, Felt};
use starknet_core::utils::get_selector_from_name;

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
    fn is_deployed(&self, address: Felt) -> Result<bool, SponsorError>;
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
pub fn plan_for(
    chain: &impl Chain,
    class_hash: Felt,
    nostr_pubkey: &str,
) -> Result<SponsorPlan, SponsorError> {
    let address = buzz_core::starknet_account::account_address(class_hash, nostr_pubkey)
        .map_err(|e| SponsorError::Config(format!("cannot derive address: {e}")))?;
    if chain.is_deployed(address)? {
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
    udc: Felt,
    class_hash: Felt,
    nostr_pubkey: &str,
    execution: &OutsideExecution,
    signature: &[Felt; 4],
) -> Result<Vec<Call>, SponsorError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// BIP-340 test vector 0's public key — a published, deliberately non-secret
    /// value, so no key was generated for these tests.
    const PUBKEY: &str = "f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9";
    const CLASS_HASH: &str = "0x0414f62ea1ed35f8c7bd3b794d94efc95e01bccf04e0f47211fc198f7f56f537";

    struct FakeChain {
        deployed: bool,
    }
    impl Chain for FakeChain {
        fn is_deployed(&self, _address: Felt) -> Result<bool, SponsorError> {
            Ok(self.deployed)
        }
    }
    struct BrokenChain;
    impl Chain for BrokenChain {
        fn is_deployed(&self, _address: Felt) -> Result<bool, SponsorError> {
            Err(SponsorError::Chain("timeout".into()))
        }
    }

    fn class_hash() -> Felt {
        Felt::from_hex_unchecked(CLASS_HASH)
    }

    #[test]
    fn undeployed_accounts_are_planned_for_deployment() {
        let plan = plan_for(&FakeChain { deployed: false }, class_hash(), PUBKEY).unwrap();
        assert!(plan.deploys());
    }

    #[test]
    fn deployed_accounts_skip_deployment() {
        let plan = plan_for(&FakeChain { deployed: true }, class_hash(), PUBKEY).unwrap();
        assert!(!plan.deploys(), "paying to redeploy would be pure waste");
    }

    #[test]
    fn a_chain_error_is_never_read_as_not_deployed() {
        // The dangerous misreading: treat a timeout as "does not exist" and pay to
        // deploy over a live account.
        assert!(matches!(
            plan_for(&BrokenChain, class_hash(), PUBKEY),
            Err(SponsorError::Chain(_))
        ));
    }

    #[test]
    fn the_planned_address_is_the_one_clients_derive() {
        // If these ever diverge, the sponsor funds an address no client can reach.
        let plan = plan_for(&FakeChain { deployed: false }, class_hash(), PUBKEY).unwrap();
        let expected = buzz_core::starknet_account::account_address(class_hash(), PUBKEY).unwrap();
        assert_eq!(plan.address(), expected);
    }

    #[test]
    fn a_bad_pubkey_is_a_config_error_not_a_panic() {
        assert!(matches!(
            plan_for(&FakeChain { deployed: false }, class_hash(), "nonsense"),
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

    #[test]
    fn different_pubkeys_get_different_accounts() {
        let other = "dff1d77f2a671c5f36183726db2341be58feae1da2deced843240f7b502ba659";
        let a = plan_for(&FakeChain { deployed: false }, class_hash(), PUBKEY).unwrap();
        let b = plan_for(&FakeChain { deployed: false }, class_hash(), other).unwrap();
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

    fn calls_for(deployed: bool) -> Vec<Call> {
        let plan = plan_for(&FakeChain { deployed }, class_hash(), PUBKEY).unwrap();
        build_atomic_calls(
            &plan,
            UDC_MAINNET,
            class_hash(),
            PUBKEY,
            &an_execution(),
            &a_signature(),
        )
        .unwrap()
    }

    #[test]
    fn an_existing_account_gets_one_call() {
        let calls = calls_for(true);
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

    #[test]
    fn a_new_account_gets_deploy_then_execute_in_that_order() {
        let calls = calls_for(false);
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

    #[test]
    fn the_deploy_call_creates_the_account_the_execute_call_targets() {
        // The two calls must agree, or the multicall deploys one account and calls
        // another. Recompute the deployed address from the deploy calldata itself.
        let calls = calls_for(false);
        let d = &calls[0].calldata;
        let recomputed =
            starknet_core::utils::get_contract_address(d[1], d[0], &d[4..], Felt::ZERO);
        assert_eq!(recomputed, calls[1].to);
    }

    #[test]
    fn the_signed_payload_is_passed_through_unaltered() {
        // The sponsor must not be able to change what executes. Whatever the user
        // signed is what goes on the wire.
        let exec = an_execution();
        let sig = a_signature();
        let calls = calls_for(true);
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
            build_atomic_calls(
                &forged,
                UDC_MAINNET,
                class_hash(),
                PUBKEY,
                &an_execution(),
                &a_signature()
            ),
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
}
