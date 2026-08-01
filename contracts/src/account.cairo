//! A Starknet account owned by a Nostr key.
//!
//! `__validate__` checks a BIP-340 Schnorr signature over the transaction hash
//! against an x-only secp256k1 public key — i.e. a Nostr pubkey. The nsec that
//! signs Nostr events therefore signs Starknet transactions, with no separate
//! Stark-curve key.
//!
//! # What the client must sign
//!
//! BIP-340 signs a byte string, while a Starknet transaction hash is a `felt252`.
//! The encoding is fixed here as the **32-byte big-endian** representation of the
//! hash. A client that encodes differently produces signatures this account
//! rejects, so [`tx_hash_bytes`] is the normative definition and
//! `signing_message` exposes it for cross-checking from off-chain code.
//!
//! # Signature layout
//!
//! `signature` is four felts: `[r_low, r_high, s_low, s_high]`. A `felt252`
//! cannot hold a `u256`, so each scalar is split low/high. Any other length is
//! rejected.
//!
//! # Cost
//!
//! One verification measures ~22.1M L2 gas (see the `bip340` test output),
//! dominated by two secp256k1 scalar multiplications plus the tagged SHA-256.
//! Every transaction from this account pays it during validation. That is orders
//! of magnitude above a Stark-curve account and is the main practical objection
//! to this design; price it before depending on it.
//!
//! # Sponsored execution (SNIP-9)
//!
//! The account embeds OpenZeppelin's `SRC9Component`, so a relayer can submit a
//! user's calls and pay the fee: `execute_from_outside_v2` takes an
//! `OutsideExecution` plus a signature, and the component checks the caller, the
//! time window and nonce replay before delegating the signature check to
//! `is_valid_signature` — i.e. to BIP-340 against the same Nostr key. **Sponsored
//! calls therefore carry exactly the same authority as direct ones, and no new
//! signing scheme is introduced.**
//!
//! This exists because a fresh account holds no STRK and every transaction from it
//! pays ~0.78 STRK of BIP-340 verification before doing anything. Without a
//! sponsor, a newly deployed account cannot act at all.
//!
//! The hash signed here is SNIP-12 typed data over the `OutsideExecution` struct,
//! **not** [`tx_hash_bytes`] over a transaction hash. Both end up in
//! `is_valid_signature`, so both are BIP-340 over the 32-byte big-endian encoding
//! of a felt — the difference is only which felt.
//!
//! # Deliberate omissions
//!
//! No owner rotation, no guardian, no session keys, no upgrade path. Each is a
//! feature whose absence is safer than a rushed implementation, and adding any of
//! them changes the security model enough to want review on its own terms.
//! Consequently **the Nostr key is the sole and permanent authority** over this
//! account: lose it and the funds are gone, leak it and they are stolen.
//!
//! Sponsorship does not soften that. A relayer chooses *whether* to pay, never
//! what executes: it cannot alter the calls, and a signature it cannot produce is
//! a signature it cannot forge.

/// SNIP-6 `is_valid_signature` success value: the `VALID` short string.
pub const VALIDATED: felt252 = 'VALID';

#[starknet::interface]
pub trait INostrAccount<TState> {
    /// The x-only Nostr public key that owns this account.
    fn get_public_key(self: @TState) -> u256;
    /// SNIP-6 signature check over an arbitrary hash.
    fn is_valid_signature(self: @TState, hash: felt252, signature: Array<felt252>) -> felt252;
    /// The exact bytes a client must BIP-340-sign for `hash`.
    ///
    /// Exposed so off-chain signers can assert they agree with the contract
    /// rather than assuming; a mismatch here is the likeliest integration bug.
    fn signing_message(self: @TState, hash: felt252) -> ByteArray;
}

/// Encodes a transaction hash as the 32-byte big-endian string BIP-340 signs.
pub fn tx_hash_bytes(hash: felt252) -> ByteArray {
    let value: u256 = hash.into();
    let mut ba: ByteArray = Default::default();
    ba.append_word(value.high.into(), 16);
    ba.append_word(value.low.into(), 16);
    ba
}

#[starknet::contract(account)]
pub mod NostrAccount {
    use openzeppelin_account::extensions::SRC9Component;
    use openzeppelin_introspection::src5::SRC5Component;
    use starknet::account::Call;
    use starknet::storage::{StoragePointerReadAccess, StoragePointerWriteAccess};
    use starknet::syscalls::call_contract_syscall;
    use starknet::{SyscallResultTrait, get_caller_address, get_tx_info};
    use crate::bip340;
    use super::{VALIDATED, tx_hash_bytes};

    // SNIP-9 outside execution, plus the SRC5 introspection it advertises through.
    // SRC9Component is account-agnostic: it requires only that this contract
    // implements SNIP-6 `is_valid_signature`, which routes to BIP-340 below. That
    // is why sponsorship needs no second signing scheme.
    component!(path: SRC5Component, storage: src5, event: SRC5Event);
    component!(path: SRC9Component, storage: src9, event: SRC9Event);

    #[abi(embed_v0)]
    impl OutsideExecutionV2Impl =
        SRC9Component::OutsideExecutionV2Impl<ContractState>;
    #[abi(embed_v0)]
    impl SRC5Impl = SRC5Component::SRC5Impl<ContractState>;

    impl SRC9InternalImpl = SRC9Component::InternalImpl<ContractState>;

    // The domain separator comes from the component's own SNIP12MetadataImpl
    // ('Account.execute_from_outside', version 2), which is what SNIP-9 mandates.
    // Defining our own would produce hashes no standard relayer computes.
    impl SNIP12MetadataImpl = SRC9Component::SNIP12MetadataImpl;

    #[storage]
    struct Storage {
        /// x-only Nostr pubkey. Immutable: there is no rotation entry point.
        public_key: u256,
        #[substorage(v0)]
        src5: SRC5Component::Storage,
        #[substorage(v0)]
        src9: SRC9Component::Storage,
    }

    #[event]
    #[derive(Drop, starknet::Event)]
    enum Event {
        #[flat]
        SRC5Event: SRC5Component::Event,
        #[flat]
        SRC9Event: SRC9Component::Event,
    }

    #[constructor]
    fn constructor(ref self: ContractState, public_key: u256) {
        // A zero key would be unspendable rather than merely wrong, and the
        // address is derived from constructor calldata, so catching it here
        // prevents funds being sent to a permanently locked account.
        assert(public_key != 0, 'nostr pubkey must be nonzero');
        self.public_key.write(public_key);
        // Registers ISRC9_V2_ID in SRC5 so a relayer can discover that this
        // account accepts sponsored calls before attempting one.
        self.src9.initializer();
    }

    #[abi(embed_v0)]
    impl NostrAccountImpl of super::INostrAccount<ContractState> {
        fn get_public_key(self: @ContractState) -> u256 {
            self.public_key.read()
        }

        fn is_valid_signature(
            self: @ContractState, hash: felt252, signature: Array<felt252>,
        ) -> felt252 {
            if self.is_valid_bip340(hash, signature.span()) {
                VALIDATED
            } else {
                // SNIP-6 leaves the failure value open; 0 is what every deployed
                // account uses and what verifiers check against.
                0
            }
        }

        fn signing_message(self: @ContractState, hash: felt252) -> ByteArray {
            tx_hash_bytes(hash)
        }
    }

    #[abi(per_item)]
    #[generate_trait]
    impl ProtocolImpl of ProtocolTrait {
        #[external(v0)]
        fn __validate__(ref self: ContractState, calls: Array<Call>) -> felt252 {
            self.only_protocol();
            self.validate_transaction()
        }

        #[external(v0)]
        fn __execute__(ref self: ContractState, calls: Array<Call>) -> Array<Span<felt252>> {
            self.only_protocol();
            execute_calls(calls)
        }

        /// Validates the deployment transaction of this account.
        ///
        /// Takes the same arguments the protocol passes for a deploy_account
        /// transaction. The signature covers the transaction hash exactly as in
        /// `__validate__`, so a counterfactually-funded account can deploy itself.
        #[external(v0)]
        fn __validate_deploy__(
            ref self: ContractState,
            class_hash: felt252,
            contract_address_salt: felt252,
            public_key: u256,
        ) -> felt252 {
            self.only_protocol();
            self.validate_transaction()
        }

        #[external(v0)]
        fn __validate_declare__(ref self: ContractState, class_hash: felt252) -> felt252 {
            self.only_protocol();
            self.validate_transaction()
        }
    }

    #[generate_trait]
    impl InternalImpl of InternalTrait {
        /// Rejects any caller other than the sequencer.
        ///
        /// Without this, another contract could invoke `__execute__` directly and
        /// run arbitrary calls with this account's authority, bypassing signature
        /// validation entirely. The protocol invokes with a zero caller address.
        fn only_protocol(self: @ContractState) {
            let caller = get_caller_address();
            assert(caller.into() == 0_felt252, 'only protocol may call');
        }

        /// Verifies the current transaction's signature.
        fn validate_transaction(self: @ContractState) -> felt252 {
            let tx_info = get_tx_info().unbox();
            if self.is_valid_bip340(tx_info.transaction_hash, tx_info.signature) {
                VALIDATED
            } else {
                // Panic rather than return 0: a non-VALIDATED return from
                // __validate__ is not a reliable rejection across protocol
                // versions, whereas a revert always is.
                panic!("invalid bip340 signature")
            }
        }

        /// BIP-340 check over `[r_low, r_high, s_low, s_high]`.
        fn is_valid_bip340(self: @ContractState, hash: felt252, signature: Span<felt252>) -> bool {
            if signature.len() != 4 {
                return false;
            }
            // A felt252 wider than u128 is malformed input, not a reason to
            // panic: is_valid_signature must answer "no", and an oversized limb
            // must not turn a read into a revert.
            let (r_low, r_high, s_low, s_high) = (
                (*signature.at(0)).try_into(),
                (*signature.at(1)).try_into(),
                (*signature.at(2)).try_into(),
                (*signature.at(3)).try_into(),
            );
            let (r_low, r_high, s_low, s_high) = match (r_low, r_high, s_low, s_high) {
                (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
                _ => { return false; },
            };
            let r = u256 { low: r_low, high: r_high };
            let s = u256 { low: s_low, high: s_high };
            bip340::verify(self.public_key.read(), r, s, tx_hash_bytes(hash))
        }
    }

    /// Dispatches each call, propagating any failure.
    ///
    /// A revert in any call reverts the whole transaction, so a multicall is
    /// all-or-nothing.
    fn execute_calls(calls: Array<Call>) -> Array<Span<felt252>> {
        let mut results = ArrayTrait::new();
        for call in calls {
            let result = call_contract_syscall(call.to, call.selector, call.calldata)
                .unwrap_syscall();
            results.append(result);
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::tx_hash_bytes;

    #[test]
    fn tx_hash_encodes_as_32_bytes_big_endian() {
        // The normative encoding: any off-chain signer must produce these bytes.
        let ba = tx_hash_bytes(1);
        assert!(ba.len() == 32, "expected 32 bytes");
        assert!(ba.at(31).unwrap() == 1, "low byte must be last");
        assert!(ba.at(0).unwrap() == 0, "high byte must be first");
    }

    #[test]
    fn tx_hash_encoding_is_injective_on_small_values() {
        // If two hashes encoded identically, a signature over one would authorise
        // the other.
        assert!(tx_hash_bytes(1) != tx_hash_bytes(2));
        assert!(tx_hash_bytes(0) != tx_hash_bytes(1));
    }
}
