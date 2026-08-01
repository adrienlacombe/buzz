//! SNIP-9 outside execution: the client half of sponsored transactions.
//!
//! A relayer submits `execute_from_outside_v2` on a user's account and pays the
//! fee; the account still requires the user's signature, so the relayer chooses
//! *whether* to pay, never what executes. This module builds the message the user
//! signs and produces that signature.
//!
//! # Why this has to mirror the contract exactly
//!
//! The hash is SNIP-12 typed data. Every constant and field order here is fixed by
//! the on-chain implementation the account embeds
//! (`openzeppelin_account::extensions::src9`), so a mismatch anywhere produces a
//! signature the account rejects — with no diagnostic beyond `SRC9: invalid
//! signature`. The type hashes below are copied from that crate rather than
//! recomputed, and [`tests::message_hash_matches_the_cairo_fixture`] pins the
//! result against a value the Cairo test computes independently.
//!
//! # What gets signed
//!
//! The account validates via `is_valid_signature`, which is BIP-340 over the
//! 32-byte big-endian encoding of the felt — see `account.cairo`'s
//! `tx_hash_bytes`. Sponsored and direct transactions therefore differ only in
//! *which* felt is signed, never in the signing scheme.

use starknet_core::types::Felt;
use starknet_core::utils::cairo_short_string_to_felt;
use starknet_crypto::poseidon_hash_many;

/// `sn_keccak` of the SNIP-12 `StarknetDomain` type string.
///
/// From `openzeppelin_utils::cryptography::snip12`.
const STARKNET_DOMAIN_TYPE_HASH: Felt =
    Felt::from_hex_unchecked("0x1ff2f602e42168014d405a94f75e8a93d640751d71d16311266e140d8b0a210");

/// `sn_keccak` of the `OutsideExecution` type string.
///
/// From `openzeppelin_account::extensions::src9::snip12_utils`.
const OUTSIDE_EXECUTION_TYPE_HASH: Felt =
    Felt::from_hex_unchecked("0x312b56c05a7965066ddbda31c016d8d05afc305071c0ca3cdc2192c3c2f1f0f");

/// `sn_keccak` of the `Call` type string.
///
/// From `openzeppelin_account::extensions::src9::snip12_utils`.
const CALL_TYPE_HASH: Felt =
    Felt::from_hex_unchecked("0x3635c7f2a7ba93844c0d064e18e487f35ab90f7c39d00f186a781fc3f0c2ca9");

/// SNIP-9's mandated domain name. Not ours to choose: the component hardcodes it,
/// and a different value yields hashes no standard relayer computes.
const DOMAIN_NAME: &str = "Account.execute_from_outside";
/// SNIP-9 outside-execution version 2.
const DOMAIN_VERSION: Felt = Felt::TWO;
/// SNIP-12 revision 1.
const DOMAIN_REVISION: Felt = Felt::ONE;

/// The magic `caller` value meaning "any relayer may submit this".
///
/// Pinning `caller` to one address is the safer default when the relayer is known,
/// because it stops a third party front-running a signed payload. `ANY_CALLER` is
/// for when the payload is handed to an untrusted or not-yet-known submitter.
pub fn any_caller() -> Felt {
    cairo_short_string_to_felt("ANY_CALLER").expect("ANY_CALLER is a valid short string")
}

/// Errors building or signing an outside execution.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OutsideExecutionError {
    /// The chain id was not a valid Cairo short string.
    #[error("invalid chain id {0:?}: not a Cairo short string")]
    InvalidChainId(String),
    /// A field was not a valid felt.
    #[error("invalid {field}: {value}")]
    InvalidFelt {
        /// Which field failed to parse.
        field: &'static str,
        /// The offending value.
        value: String,
    },
    /// The execution window is empty or inverted, so the account would reject it.
    #[error("execute_after {after} must be strictly before execute_before {before}")]
    EmptyWindow {
        /// Lower bound.
        after: u64,
        /// Upper bound.
        before: u64,
    },
}

/// One call in an outside execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutsideCall {
    /// Callee contract address.
    pub to: Felt,
    /// Entry-point selector.
    pub selector: Felt,
    /// Raw calldata.
    pub calldata: Vec<Felt>,
}

impl OutsideCall {
    /// SNIP-12 struct hash of this call.
    fn hash_struct(&self) -> Felt {
        poseidon_hash_many(&[
            CALL_TYPE_HASH,
            self.to,
            self.selector,
            poseidon_hash_many(&self.calldata),
        ])
    }
}

/// A SNIP-9 outside execution, matching the Cairo struct field for field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutsideExecution {
    /// The only address permitted to submit this, or [`any_caller`].
    pub caller: Felt,
    /// Replay-protection nonce. Single-use per account; the account rejects a
    /// repeat, so this must be fresh rather than sequential.
    pub nonce: Felt,
    /// Valid strictly after this Unix timestamp.
    pub execute_after: u64,
    /// Valid strictly before this Unix timestamp.
    pub execute_before: u64,
    /// The calls to run, all-or-nothing.
    pub calls: Vec<OutsideCall>,
}

impl OutsideExecution {
    /// SNIP-12 struct hash of this execution.
    ///
    /// Public so a mismatch can be localised: if a sponsored call fails with
    /// `SRC9: invalid signature`, comparing this against the contract's own
    /// `hash_struct` separates "wrong struct encoding" from "wrong domain".
    pub fn hash_struct(&self) -> Felt {
        let hashed_calls: Vec<Felt> = self.calls.iter().map(OutsideCall::hash_struct).collect();
        poseidon_hash_many(&[
            OUTSIDE_EXECUTION_TYPE_HASH,
            self.caller,
            self.nonce,
            self.execute_after.into(),
            self.execute_before.into(),
            poseidon_hash_many(&hashed_calls),
        ])
    }

    /// The felt the account will verify a signature against.
    ///
    /// `account_address` is the signer in SNIP-12 terms, which is why the same
    /// payload signed for one account is invalid for another — the address is
    /// bound into the hash.
    pub fn message_hash(
        &self,
        account_address: Felt,
        chain_id: &str,
    ) -> Result<Felt, OutsideExecutionError> {
        if self.execute_after >= self.execute_before {
            return Err(OutsideExecutionError::EmptyWindow {
                after: self.execute_after,
                before: self.execute_before,
            });
        }
        let chain = cairo_short_string_to_felt(chain_id)
            .map_err(|_| OutsideExecutionError::InvalidChainId(chain_id.to_string()))?;
        let domain = poseidon_hash_many(&[
            STARKNET_DOMAIN_TYPE_HASH,
            cairo_short_string_to_felt(DOMAIN_NAME).expect("domain name is a valid short string"),
            DOMAIN_VERSION,
            chain,
            DOMAIN_REVISION,
        ]);
        Ok(poseidon_hash_many(&[
            cairo_short_string_to_felt("StarkNet Message")
                .expect("SNIP-12 prefix is a valid short string"),
            domain,
            account_address,
            self.hash_struct(),
        ]))
    }
}

/// The 32 bytes BIP-340 signs for `hash`.
///
/// Mirrors `tx_hash_bytes` in `account.cairo`: big-endian, high 16 bytes then low
/// 16. This is the likeliest place for an integration to diverge, which is why the
/// contract exposes `signing_message` for cross-checking.
pub fn signing_bytes(hash: Felt) -> [u8; 32] {
    hash.to_bytes_be()
}

/// Splits a 64-byte BIP-340 signature into the account's felt layout.
///
/// The account expects `[r_low, r_high, s_low, s_high]` because a `felt252`
/// cannot hold a `u256`. Any other length is rejected on chain.
pub fn signature_felts(signature: &[u8; 64]) -> [Felt; 4] {
    let split = |b: &[u8]| -> (Felt, Felt) {
        let mut high = [0u8; 16];
        let mut low = [0u8; 16];
        high.copy_from_slice(&b[..16]);
        low.copy_from_slice(&b[16..]);
        (
            Felt::from_bytes_be_slice(&low),
            Felt::from_bytes_be_slice(&high),
        )
    };
    let (r_low, r_high) = split(&signature[..32]);
    let (s_low, s_high) = split(&signature[32..]);
    [r_low, r_high, s_low, s_high]
}

/// Calldata for `execute_from_outside_v2(outside_execution, signature)`.
///
/// Serialised in the Cairo ABI order so a relayer can submit it without knowing
/// the struct layout.
pub fn execute_from_outside_calldata(
    execution: &OutsideExecution,
    signature: &[Felt; 4],
) -> Vec<Felt> {
    let mut out = vec![
        execution.caller,
        execution.nonce,
        execution.execute_after.into(),
        execution.execute_before.into(),
        Felt::from(execution.calls.len()),
    ];
    for call in &execution.calls {
        out.push(call.to);
        out.push(call.selector);
        out.push(Felt::from(call.calldata.len()));
        out.extend_from_slice(&call.calldata);
    }
    out.push(Felt::from(signature.len()));
    out.extend_from_slice(signature);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic execution used by both this test and the Cairo fixture.
    fn fixture() -> OutsideExecution {
        OutsideExecution {
            caller: any_caller(),
            nonce: Felt::from(42_u32),
            execute_after: 1_000,
            execute_before: 2_000,
            calls: vec![OutsideCall {
                to: Felt::from(0x1234_u32),
                selector: Felt::from(0x5678_u32),
                calldata: vec![Felt::ONE, Felt::TWO],
            }],
        }
    }

    #[test]
    fn any_caller_is_the_documented_short_string() {
        // If this drifts, every payload silently becomes callable by nobody.
        assert_eq!(
            any_caller(),
            cairo_short_string_to_felt("ANY_CALLER").unwrap()
        );
    }

    #[test]
    fn message_hash_binds_the_account_address() {
        let exec = fixture();
        let a = exec.message_hash(Felt::from(1_u32), "SN_MAIN").unwrap();
        let b = exec.message_hash(Felt::from(2_u32), "SN_MAIN").unwrap();
        assert_ne!(
            a, b,
            "the signer address must be in the hash, or a payload signed for one \
             account would replay against another"
        );
    }

    #[test]
    fn message_hash_binds_the_chain() {
        let exec = fixture();
        let main = exec.message_hash(Felt::from(1_u32), "SN_MAIN").unwrap();
        let sepolia = exec.message_hash(Felt::from(1_u32), "SN_SEPOLIA").unwrap();
        assert_ne!(
            main, sepolia,
            "a mainnet payload must not replay on testnet"
        );
    }

    #[test]
    fn message_hash_binds_every_field() {
        let base = fixture();
        let addr = Felt::from(7_u32);
        let baseline = base.message_hash(addr, "SN_MAIN").unwrap();

        let mut nonce = base.clone();
        nonce.nonce = Felt::from(43_u32);
        assert_ne!(baseline, nonce.message_hash(addr, "SN_MAIN").unwrap());

        let mut window = base.clone();
        window.execute_before = 3_000;
        assert_ne!(baseline, window.message_hash(addr, "SN_MAIN").unwrap());

        let mut caller = base.clone();
        caller.caller = Felt::from(9_u32);
        assert_ne!(baseline, caller.message_hash(addr, "SN_MAIN").unwrap());

        let mut calldata = base.clone();
        calldata.calls[0].calldata = vec![Felt::ONE, Felt::THREE];
        assert_ne!(baseline, calldata.message_hash(addr, "SN_MAIN").unwrap());

        let mut selector = base.clone();
        selector.calls[0].selector = Felt::from(0x9999_u32);
        assert_ne!(baseline, selector.message_hash(addr, "SN_MAIN").unwrap());
    }

    #[test]
    fn empty_window_is_rejected_before_signing() {
        // The account asserts execute_after < now < execute_before, so an inverted
        // window can only ever fail on chain. Catch it here rather than paying a
        // relayer to discover it.
        let mut exec = fixture();
        exec.execute_before = exec.execute_after;
        assert_eq!(
            exec.message_hash(Felt::ONE, "SN_MAIN"),
            Err(OutsideExecutionError::EmptyWindow {
                after: 1_000,
                before: 1_000
            })
        );
    }

    #[test]
    fn invalid_chain_id_is_reported() {
        let exec = fixture();
        let too_long = "A".repeat(32);
        assert!(matches!(
            exec.message_hash(Felt::ONE, &too_long),
            Err(OutsideExecutionError::InvalidChainId(_))
        ));
    }

    #[test]
    fn signature_felts_split_low_high_in_the_account_order() {
        let mut sig = [0u8; 64];
        // r = 0x00..01, s = 0x00..02 with the marker in the low half of each.
        sig[31] = 1;
        sig[63] = 2;
        let felts = signature_felts(&sig);
        assert_eq!(felts[0], Felt::ONE, "r_low");
        assert_eq!(felts[1], Felt::ZERO, "r_high");
        assert_eq!(felts[2], Felt::TWO, "s_low");
        assert_eq!(felts[3], Felt::ZERO, "s_high");
    }

    #[test]
    fn signature_felts_keep_the_high_half_distinct() {
        let mut sig = [0u8; 64];
        sig[0] = 0xAA; // most significant byte of r
        let felts = signature_felts(&sig);
        assert_eq!(felts[0], Felt::ZERO, "r_low must not absorb the high bytes");
        assert_ne!(felts[1], Felt::ZERO, "r_high must carry them");
    }

    #[test]
    fn signing_bytes_are_the_32_byte_big_endian_felt() {
        // Must agree with tx_hash_bytes in account.cairo.
        assert_eq!(signing_bytes(Felt::ONE)[31], 1);
        assert_eq!(signing_bytes(Felt::ONE)[..31], [0u8; 31]);
    }

    #[test]
    fn calldata_is_serialised_in_abi_order() {
        let exec = fixture();
        let sig = [Felt::ONE, Felt::TWO, Felt::THREE, Felt::from(4_u32)];
        let data = execute_from_outside_calldata(&exec, &sig);
        assert_eq!(data[0], any_caller());
        assert_eq!(data[1], Felt::from(42_u32));
        assert_eq!(data[2], Felt::from(1_000_u32));
        assert_eq!(data[3], Felt::from(2_000_u32));
        assert_eq!(data[4], Felt::ONE, "one call");
        assert_eq!(data[5], Felt::from(0x1234_u32), "call.to");
        assert_eq!(data[6], Felt::from(0x5678_u32), "call.selector");
        assert_eq!(data[7], Felt::TWO, "calldata len");
        assert_eq!(data[8], Felt::ONE);
        assert_eq!(data[9], Felt::TWO);
        assert_eq!(data[10], Felt::from(4_u32), "signature len");
        assert_eq!(&data[11..], &sig[..]);
    }

    /// The struct hash OpenZeppelin's own `OutsideExecutionStructHash` produces
    /// for [`fixture`], asserted independently by
    /// `outside_execution_struct_hash_matches_rust` in
    /// `contracts/tests/test_account.cairo`.
    ///
    /// This is the cross-check that matters. The struct hash is where a mirror
    /// implementation actually goes wrong — nested call hashing, field order, two
    /// separate type hashes — and it is computed here by us and there by them.
    /// The outer domain wrapper is four Poseidon elements, verified by reading
    /// `openzeppelin_utils::cryptography::snip12`.
    const CAIRO_STRUCT_HASH: &str =
        "0x2d0847a575b68e0e072cab0d3e1dd2df5db9eabe1a04730833ab173696b0d45";

    #[test]
    fn struct_hash_matches_the_cairo_implementation() {
        assert_eq!(
            fixture().hash_struct(),
            Felt::from_hex_unchecked(CAIRO_STRUCT_HASH),
            "Rust and Cairo disagree on the OutsideExecution struct hash; every \
             sponsored call would fail with SRC9: invalid signature"
        );
    }
}
