//! Account integration tests, structured after OpenZeppelin's `test_account.cairo`.
//!
//! The unit tests in `src/bip340.cairo` prove the verifier against the official
//! BIP-340 vectors. These prove the *account* around it: that the protocol gate
//! holds, that `__validate__` accepts only a real signature, that `__execute__`
//! dispatches, and that malformed signatures are answered rather than panicking.
//!
//! # Where the real signature comes from
//!
//! An end-to-end test needs a genuine BIP-340 signature over a `felt252`
//! transaction hash. BIP-340 vector 0 signs a 32-zero-byte message, and
//! `tx_hash_bytes(0)` produces exactly 32 zero bytes — so that vector's signature
//! is valid for transaction hash `0` under its own public key. That is the only
//! official vector usable this way: the others sign 32-byte messages that exceed
//! the felt252 modulus and so cannot be a Starknet transaction hash.

use buzz_starknet::account::{
    INostrAccountDispatcher, INostrAccountDispatcherTrait, VALIDATED, tx_hash_bytes,
};
use buzz_starknet::mocks::{ISimpleMockDispatcher, ISimpleMockDispatcherTrait};
use openzeppelin_account::extensions::src9::OutsideExecution;
use openzeppelin_account::extensions::src9::snip12_utils::OutsideExecutionStructHash;
use openzeppelin_utils::cryptography::snip12::StructHash;
use snforge_std::{
    ContractClassTrait, DeclareResultTrait, declare, start_cheat_caller_address,
    start_cheat_signature_global, start_cheat_transaction_hash_global,
};
use starknet::account::Call;
use starknet::{ContractAddress, SyscallResultTrait};

/// BIP-340 vector 0: public key.
const PUBKEY_X: u256 = 0xf9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9;
/// BIP-340 vector 0: signature R.x, valid over a 32-zero-byte message.
const SIG_R: u256 = 0xe907831f80848d1069a5371b402410364bdf1c5f8307b0084c55f1ce2dca8215;
/// BIP-340 vector 0: signature s.
const SIG_S: u256 = 0x25f66a4a85ea8b71e482a74f382d2ce5ebeee8fdb2172f477df4900d310536c0;
/// The transaction hash the above signature authorises.
const SIGNED_TX_HASH: felt252 = 0;

/// Signature in the account's wire layout: `[r_low, r_high, s_low, s_high]`.
fn valid_signature() -> Array<felt252> {
    array![SIG_R.low.into(), SIG_R.high.into(), SIG_S.low.into(), SIG_S.high.into()]
}

fn deploy_account() -> INostrAccountDispatcher {
    let contract = declare("NostrAccount").unwrap().contract_class();
    let mut calldata = array![];
    calldata.append(PUBKEY_X.low.into());
    calldata.append(PUBKEY_X.high.into());
    let (address, _) = contract.deploy(@calldata).unwrap();
    INostrAccountDispatcher { contract_address: address }
}

fn deploy_mock() -> ISimpleMockDispatcher {
    let contract = declare("SimpleMock").unwrap().contract_class();
    let (address, _) = contract.deploy(@array![]).unwrap();
    ISimpleMockDispatcher { contract_address: address }
}

/// Puts the account in the state the sequencer would: protocol as caller, with
/// the signed transaction hash and signature in the transaction context.
fn cheat_protocol_call(account: ContractAddress) {
    start_cheat_signature_global(valid_signature().span());
    start_cheat_transaction_hash_global(SIGNED_TX_HASH);
    start_cheat_caller_address(account, 0.try_into().unwrap());
}

//
// is_valid_signature — SNIP-6
//

#[test]
fn is_valid_signature_accepts_a_real_bip340_signature() {
    let account = deploy_account();
    assert_eq!(account.is_valid_signature(SIGNED_TX_HASH, valid_signature()), VALIDATED);
}

#[test]
fn is_valid_signature_rejects_a_different_hash() {
    // The same signature must not authorise another transaction.
    let account = deploy_account();
    assert_eq!(account.is_valid_signature(1, valid_signature()), 0);
}

#[test]
fn is_valid_signature_rejects_a_tampered_signature() {
    let account = deploy_account();
    let mut sig = valid_signature();
    let bad = array![*sig.at(0) + 1, *sig.at(1), *sig.at(2), *sig.at(3)];
    assert_eq!(account.is_valid_signature(SIGNED_TX_HASH, bad), 0);
}

#[test]
fn is_valid_signature_rejects_wrong_length() {
    let account = deploy_account();
    assert_eq!(account.is_valid_signature(SIGNED_TX_HASH, array![1, 2, 3]), 0);
    assert_eq!(account.is_valid_signature(SIGNED_TX_HASH, array![1, 2, 3, 4, 5]), 0);
}

#[test]
fn is_valid_signature_rejects_empty_signature() {
    let account = deploy_account();
    assert_eq!(account.is_valid_signature(SIGNED_TX_HASH, array![]), 0);
}

#[test]
fn is_valid_signature_answers_no_for_oversized_limbs() {
    // A limb wider than u128 is malformed input. It must return 0 rather than
    // revert: a verifier calling this read cannot distinguish a panic from an
    // infrastructure failure.
    let account = deploy_account();
    let too_big: felt252 = 0x1000000000000000000000000000000000; // > 2^128
    assert_eq!(account.is_valid_signature(SIGNED_TX_HASH, array![too_big, 0, 0, 0]), 0);
}

//
// Protocol entry points
//

#[test]
fn validate_accepts_the_signed_transaction() {
    let account = deploy_account();
    cheat_protocol_call(account.contract_address);
    let dispatcher = IProtocolDispatcher { contract_address: account.contract_address };
    assert_eq!(dispatcher.__validate__(array![]), VALIDATED);
}

#[test]
#[should_panic(expected: "invalid bip340 signature")]
fn validate_rejects_an_unsigned_transaction() {
    let account = deploy_account();
    start_cheat_signature_global(valid_signature().span());
    // A hash the signature does not cover.
    start_cheat_transaction_hash_global(1);
    start_cheat_caller_address(account.contract_address, 0.try_into().unwrap());
    let dispatcher = IProtocolDispatcher { contract_address: account.contract_address };
    dispatcher.__validate__(array![]);
}

#[test]
fn validate_declare_accepts_the_signed_transaction() {
    let account = deploy_account();
    cheat_protocol_call(account.contract_address);
    let dispatcher = IProtocolDispatcher { contract_address: account.contract_address };
    assert_eq!(dispatcher.__validate_declare__(0x1234), VALIDATED);
}

#[test]
fn validate_deploy_accepts_the_signed_transaction() {
    let account = deploy_account();
    cheat_protocol_call(account.contract_address);
    let dispatcher = IProtocolDispatcher { contract_address: account.contract_address };
    assert_eq!(dispatcher.__validate_deploy__(0x1234, 0x5678, PUBKEY_X), VALIDATED);
}

//
// The protocol gate — the failure that would be catastrophic
//

#[test]
#[should_panic(expected: 'only protocol may call')]
fn execute_rejects_a_contract_caller() {
    // Without only_protocol, any contract could drive __execute__ with this
    // account's authority and skip signature validation entirely.
    let account = deploy_account();
    let attacker: ContractAddress = 0xbad.try_into().unwrap();
    start_cheat_caller_address(account.contract_address, attacker);
    let dispatcher = IProtocolDispatcher { contract_address: account.contract_address };
    dispatcher.__execute__(array![]);
}

#[test]
#[should_panic(expected: 'only protocol may call')]
fn validate_rejects_a_contract_caller() {
    let account = deploy_account();
    let attacker: ContractAddress = 0xbad.try_into().unwrap();
    start_cheat_caller_address(account.contract_address, attacker);
    let dispatcher = IProtocolDispatcher { contract_address: account.contract_address };
    dispatcher.__validate__(array![]);
}

//
// Execution
//

#[test]
fn execute_dispatches_a_call() {
    let account = deploy_account();
    let mock = deploy_mock();
    cheat_protocol_call(account.contract_address);

    let call = Call {
        to: mock.contract_address, selector: selector!("set_value"), calldata: array![42].span(),
    };
    let dispatcher = IProtocolDispatcher { contract_address: account.contract_address };
    dispatcher.__execute__(array![call]);

    assert_eq!(mock.get_value(), 42);
}

#[test]
fn execute_runs_a_multicall_in_order() {
    let account = deploy_account();
    let mock = deploy_mock();
    cheat_protocol_call(account.contract_address);

    let first = Call {
        to: mock.contract_address, selector: selector!("set_value"), calldata: array![1].span(),
    };
    let second = Call {
        to: mock.contract_address, selector: selector!("set_value"), calldata: array![2].span(),
    };
    let dispatcher = IProtocolDispatcher { contract_address: account.contract_address };
    let results = dispatcher.__execute__(array![first, second]);

    assert_eq!(results.len(), 2);
    // Last write wins, so ordering is observable.
    assert_eq!(mock.get_value(), 2);
}

#[test]
#[should_panic]
fn execute_reverts_the_whole_multicall_when_one_call_fails() {
    // All-or-nothing: a later failure must undo the earlier write.
    let account = deploy_account();
    let mock = deploy_mock();
    cheat_protocol_call(account.contract_address);

    let ok = Call {
        to: mock.contract_address, selector: selector!("set_value"), calldata: array![7].span(),
    };
    let boom = Call {
        to: mock.contract_address, selector: selector!("always_panics"), calldata: array![].span(),
    };
    let dispatcher = IProtocolDispatcher { contract_address: account.contract_address };
    dispatcher.__execute__(array![ok, boom]);
}

//
// Constructor and accessors
//

#[test]
fn get_public_key_returns_the_constructor_value() {
    let account = deploy_account();
    assert_eq!(account.get_public_key(), PUBKEY_X);
}

#[test]
#[should_panic(expected: 'nostr pubkey must be nonzero')]
fn constructor_rejects_a_zero_public_key() {
    // The address derives from constructor calldata, so a zero key would be a
    // fundable but permanently locked account.
    let contract = declare("NostrAccount").unwrap().contract_class();
    contract.deploy(@array![0, 0]).unwrap_syscall();
}

#[test]
fn signing_message_matches_the_normative_encoding() {
    // Off-chain signers must agree with the contract byte for byte; this is the
    // likeliest place for an integration bug.
    let account = deploy_account();
    assert_eq!(account.signing_message(0), tx_hash_bytes(0));
    assert_eq!(account.signing_message(0x1234), tx_hash_bytes(0x1234));
}

/// Protocol entry points are not part of `INostrAccount` — they are invoked by
/// the sequencer, so they need their own dispatcher for tests to call them.
#[starknet::interface]
trait IProtocol<TState> {
    fn __validate__(ref self: TState, calls: Array<Call>) -> felt252;
    fn __execute__(ref self: TState, calls: Array<Call>) -> Array<Span<felt252>>;
    fn __validate_declare__(ref self: TState, class_hash: felt252) -> felt252;
    fn __validate_deploy__(
        ref self: TState, class_hash: felt252, contract_address_salt: felt252, public_key: u256,
    ) -> felt252;
}

//
// SNIP-9 outside execution (sponsored calls)
//
// The signature check is SRC9Component delegating to `is_valid_signature`, i.e.
// the same BIP-340 path the direct tests above cover. What is worth testing here
// is the surface SRC9 adds around it: interface discovery, nonce replay, the time
// window, and the caller restriction.
//
// The accepting path needs a BIP-340 signature over a SNIP-12 hash that depends on
// the account's own address and the chain id, so its fixture has to be produced
// off-chain. It is generated by the relayer client rather than hand-written here —
// see `buzz-core`'s outside-execution helpers.

/// `ISRC9_V2_ID`, the SRC5 interface id SRC9Component registers. A relayer reads
/// this to discover the account accepts sponsored calls before paying for one.
const ISRC9_V2_ID: felt252 = 0x1d1144bb2138366ff28d8e9ab57456b1d332ac42196230c3a602003c89872;

#[starknet::interface]
trait ISrc5Probe<TState> {
    fn supports_interface(self: @TState, interface_id: felt252) -> bool;
}

#[starknet::interface]
trait ISrc9Probe<TState> {
    fn is_valid_outside_execution_nonce(self: @TState, nonce: felt252) -> bool;
}

#[test]
fn advertises_the_snip9_interface() {
    let account = deploy_account();
    let probe = ISrc5ProbeDispatcher { contract_address: account.contract_address };
    assert!(
        probe.supports_interface(ISRC9_V2_ID),
        "the constructor must register ISRC9_V2 or no relayer will offer to sponsor",
    );
}

#[test]
fn unused_nonces_start_available() {
    let account = deploy_account();
    let probe = ISrc9ProbeDispatcher { contract_address: account.contract_address };
    assert!(probe.is_valid_outside_execution_nonce(1), "a fresh nonce must be available");
    assert!(probe.is_valid_outside_execution_nonce(0), "zero is a nonce like any other");
}

/// Cross-checks the Rust client's SNIP-12 struct hash against OpenZeppelin's.
///
/// The struct hash is where a mirror implementation actually goes wrong: nested
/// call hashing, field order, and two separate type hashes. This computes it with
/// OZ's own `OutsideExecutionStructHash` for the same fixture
/// `buzz_core::outside_execution::tests::fixture` uses, so the two are pinned
/// together. A disagreement means every sponsored transaction fails with
/// `SRC9: invalid signature` and nothing says why.
///
/// To regenerate after changing the fixture: run this test, take the actual value
/// from the failure, and update `CAIRO_STRUCT_HASH` in `outside_execution.rs`.
#[test]
fn outside_execution_struct_hash_matches_rust() {
    let calls = array![
        Call {
            to: 0x1234.try_into().unwrap(), selector: 0x5678, calldata: array![1, 2].span(),
        },
    ];
    let execution = OutsideExecution {
        caller: 'ANY_CALLER'.try_into().unwrap(),
        nonce: 42,
        execute_after: 1000,
        execute_before: 2000,
        calls: calls.span(),
    };
    assert_eq!(
        execution.hash_struct(),
        0x2d0847a575b68e0e072cab0d3e1dd2df5db9eabe1a04730833ab173696b0d45,
        "OZ's struct hash must equal the one buzz-core computes",
    );
}
