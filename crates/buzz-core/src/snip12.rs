//! SNIP-12 typed data for NIP-SW wallet-binding attestations.
//!
//! See `docs/nips/NIP-SW.md`. This module builds the SNIP-12 document a user's
//! Starknet account must sign to attest a binding, and derives its message hash.
//! It is pure computation — no I/O, no network, no key material. Nothing here
//! signs anything.
//!
//! # Why derivation, not validation
//!
//! An earlier design had the client submit the signed message hash and the relay
//! merely check `is_valid_signature(submitted_hash, signature)`. That is
//! trivially forgeable: Starknet transaction signatures are public, so an
//! attacker can lift any `(tx_hash, signature)` pair from a victim's account
//! off-chain data and publish it as an attestation under their own Nostr
//! identity. The account would confirm the signature is valid — because it is —
//! over a message that says nothing about the binding.
//!
//! The hash is therefore **derived** from the binding's own fields and never
//! accepted from the submitter.
//!
//! # One document, both sides
//!
//! [`BindingMessage::typed_data_json`] produces the SNIP-12 document. A client
//! hands that exact document to the wallet for signing; the relay rebuilds the
//! identical document from the event and hashes it. Both sides use
//! `starknet-core`'s SNIP-12 implementation, so wallet compatibility does not
//! depend on this crate hand-rolling type strings or encodings.

use serde::{Deserialize, Serialize};
use starknet_core::types::typed_data::TypedData;
use starknet_crypto::Felt;

/// SNIP-12 revision. Revision 1 hashes with Poseidon.
pub const REVISION: &str = "1";

/// Domain name for NIP-SW attestations.
///
/// Part of the SNIP-12 domain separator: a signature produced for this domain
/// cannot be replayed against another application's typed data.
pub const DOMAIN_NAME: &str = "Buzz NIP-SW";

/// Domain version.
pub const DOMAIN_VERSION: &str = "1";

/// SNIP-12 primary type name for a binding attestation.
pub const PRIMARY_TYPE: &str = "NostrWalletBinding";

/// The SNIP-6 `is_valid_signature` entry-point selector.
///
/// `starknet_keccak("is_valid_signature")`. Stated as a constant so callers need
/// no fallible selector computation on the ingest hot path. Pinned by
/// `is_valid_signature_selector_matches_independent_implementation`, which
/// checks it against `starknet-core`'s `get_selector_from_name` — itself
/// cross-checked against two independent keccak implementations.
pub const IS_VALID_SIGNATURE_SELECTOR: &str =
    "0x028420862938116cb3bbdbedee07451ccc54d4e9412dbef71142ad1980a30941";

/// Errors from building or hashing a binding attestation message.
#[derive(Debug, thiserror::Error)]
pub enum Snip12Error {
    /// The Nostr pubkey was not 32 bytes of hex.
    #[error("nostr pubkey must be 32 bytes of hex, got {0}")]
    InvalidPubkey(String),
    /// The account address was not a valid felt.
    #[error("invalid account address: {0}")]
    InvalidAddress(String),
    /// The typed-data document was rejected by the SNIP-12 encoder.
    #[error("invalid typed data: {0}")]
    TypedData(String),
}

/// The fields a binding attestation commits to.
///
/// Every field is load-bearing. The pubkey makes the attestation directional —
/// without it, a signature over the address alone is replayable by anyone who
/// observed it. The chain id stops a mainnet attestation being reused for a
/// testnet binding. `signed_at` lets verifiers reason about age, which matters
/// because account ownership is rotatable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingMessage<'a> {
    /// The binding author's Nostr pubkey, 32 bytes hex.
    pub nostr_pubkey: &'a str,
    /// Starknet chain id short string, e.g. `SN_MAIN`.
    pub chain_id: &'a str,
    /// Unix seconds the attestation was produced.
    pub signed_at: u64,
    /// The account contract address that signs, felt hex.
    pub account_address: &'a str,
}

/// Split a 32-byte hex Nostr pubkey into `(high, low)` 16-byte halves as hex.
///
/// A 32-byte value can exceed the felt252 modulus, so it cannot be a single
/// field element. Two 128-bit halves always fit.
///
/// Two plain `felt` fields are used rather than the SNIP-12 `u256` preset:
/// `felt` is universally supported by signing wallets, whereas relying on preset
/// handling adds a compatibility dependency for no benefit.
fn split_pubkey(pubkey_hex: &str) -> Result<(String, String), Snip12Error> {
    let raw = pubkey_hex.strip_prefix("0x").unwrap_or(pubkey_hex);
    let bytes =
        hex::decode(raw).map_err(|_| Snip12Error::InvalidPubkey("not valid hex".to_string()))?;
    if bytes.len() != 32 {
        return Err(Snip12Error::InvalidPubkey(format!("{} bytes", bytes.len())));
    }
    Ok((
        format!("0x{}", hex::encode(&bytes[..16])),
        format!("0x{}", hex::encode(&bytes[16..])),
    ))
}

impl BindingMessage<'_> {
    /// Build the SNIP-12 typed-data document.
    ///
    /// This is the artifact to hand a wallet for signing. The relay rebuilds it
    /// byte-identically from the stored event, so both sides hash the same
    /// document.
    pub fn typed_data_json(&self) -> Result<serde_json::Value, Snip12Error> {
        let (pubkey_high, pubkey_low) = split_pubkey(self.nostr_pubkey)?;
        Ok(serde_json::json!({
            "types": {
                "StarknetDomain": [
                    { "name": "name",     "type": "shortstring" },
                    { "name": "version",  "type": "shortstring" },
                    { "name": "chainId",  "type": "shortstring" },
                    { "name": "revision", "type": "shortstring" }
                ],
                PRIMARY_TYPE: [
                    { "name": "nostrPubkeyHigh", "type": "felt" },
                    { "name": "nostrPubkeyLow",  "type": "felt" },
                    { "name": "chainId",         "type": "shortstring" },
                    { "name": "signedAt",        "type": "felt" }
                ]
            },
            "primaryType": PRIMARY_TYPE,
            "domain": {
                "name": DOMAIN_NAME,
                "version": DOMAIN_VERSION,
                "chainId": self.chain_id,
                "revision": REVISION
            },
            "message": {
                "nostrPubkeyHigh": pubkey_high,
                "nostrPubkeyLow": pubkey_low,
                "chainId": self.chain_id,
                "signedAt": self.signed_at
            }
        }))
    }

    /// Parse the document into a [`TypedData`].
    pub fn typed_data(&self) -> Result<TypedData, Snip12Error> {
        let json = self.typed_data_json()?;
        serde_json::from_value(json).map_err(|e| Snip12Error::TypedData(e.to_string()))
    }

    /// Derive the SNIP-12 message hash the account must sign.
    pub fn message_hash(&self) -> Result<Felt, Snip12Error> {
        let address = Felt::from_hex(self.account_address)
            .map_err(|e| Snip12Error::InvalidAddress(e.to_string()))?;
        self.typed_data()?
            .message_hash(address)
            .map_err(|e| Snip12Error::TypedData(e.to_string()))
    }

    /// Derive the message hash as a `0x` hex string.
    pub fn message_hash_hex(&self) -> Result<String, Snip12Error> {
        Ok(self.message_hash()?.to_hex_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use starknet_core::utils::get_selector_from_name;

    const PUBKEY: &str = "953d1b1c0e0e0a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f70819200";
    const ADDRESS: &str = "0x04a5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f80919a2b";

    fn message() -> BindingMessage<'static> {
        BindingMessage {
            nostr_pubkey: PUBKEY,
            chain_id: "SN_MAIN",
            signed_at: 1_785_400_000,
            account_address: ADDRESS,
        }
    }

    #[test]
    fn is_valid_signature_selector_matches_independent_implementation() {
        // Cross-check starknet-core's selector against a value computed by two
        // independent keccak implementations (pycryptodome and eth_hash, which
        // agree):
        //   keccak256("is_valid_signature")
        //     = ae8420862938116cb3bbdbedee07451ccc54d4e9412dbef71142ad1980a30941
        // starknet_keccak clears the top 6 bits, so 0xae & 0x03 = 0x02.
        //
        // This is the SNIP-6 entry point the relay calls. A wrong value means
        // every verification hits the wrong function.
        let selector = get_selector_from_name("is_valid_signature").expect("ascii name");
        assert_eq!(
            selector.to_fixed_hex_string(),
            "0x028420862938116cb3bbdbedee07451ccc54d4e9412dbef71142ad1980a30941"
        );
        // And the constant the relay actually uses agrees with both.
        assert_eq!(
            Felt::from_hex(IS_VALID_SIGNATURE_SELECTOR).expect("parse constant"),
            selector
        );
    }

    #[test]
    fn typed_data_document_parses() {
        // If the document shape were wrong, the encoder would reject it here
        // rather than silently producing a hash no wallet agrees with.
        message().typed_data().expect("valid SNIP-12 document");
    }

    #[test]
    fn document_declares_revision_1() {
        let json = message().typed_data_json().expect("json");
        assert_eq!(json["domain"]["revision"], REVISION);
        assert_eq!(json["primaryType"], PRIMARY_TYPE);
    }

    #[test]
    fn pubkey_splits_into_two_halves() {
        let (high, low) = split_pubkey(PUBKEY).expect("split");
        // 16 bytes each => 32 hex chars plus the 0x prefix.
        assert_eq!(high.len(), 34);
        assert_eq!(low.len(), 34);
        assert_eq!(format!("{}{}", &high[2..], &low[2..]), PUBKEY);
    }

    #[test]
    fn pubkey_rejects_wrong_length() {
        assert!(matches!(
            split_pubkey("abcd"),
            Err(Snip12Error::InvalidPubkey(_))
        ));
        assert!(matches!(
            split_pubkey("zz".repeat(32).as_str()),
            Err(Snip12Error::InvalidPubkey(_))
        ));
    }

    #[test]
    fn message_hash_is_deterministic() {
        let a = message().message_hash().expect("hash");
        let b = message().message_hash().expect("hash");
        assert_eq!(a, b);
    }

    #[test]
    fn message_hash_binds_every_field() {
        // If any field could change without changing the hash, an attestation
        // for one binding would verify for another.
        let base = message().message_hash().expect("hash");

        let flipped = format!("0{}", &PUBKEY[1..]);
        let mut other_pubkey = message();
        other_pubkey.nostr_pubkey = &flipped;
        assert_ne!(base, other_pubkey.message_hash().expect("hash"));

        let mut other_chain = message();
        other_chain.chain_id = "SN_SEPOLIA";
        assert_ne!(base, other_chain.message_hash().expect("hash"));

        let mut other_time = message();
        other_time.signed_at += 1;
        assert_ne!(base, other_time.message_hash().expect("hash"));

        // The address is mixed in by SNIP-12 itself, not by the message struct.
        let mut other_account = message();
        other_account.account_address = "0x1";
        assert_ne!(base, other_account.message_hash().expect("hash"));
    }

    #[test]
    fn rejects_malformed_account_address() {
        let mut m = message();
        m.account_address = "not-hex";
        assert!(matches!(
            m.message_hash(),
            Err(Snip12Error::InvalidAddress(_))
        ));
    }

    #[test]
    fn hex_output_parses_back_to_the_same_felt() {
        let hex = message().message_hash_hex().expect("hash");
        assert!(hex.starts_with("0x"));
        let reparsed = Felt::from_hex(&hex).expect("parse");
        assert_eq!(reparsed, message().message_hash().expect("hash"));
    }

    /// Wallet-compatibility vector.
    ///
    /// The tests above prove the document is well-formed, the hash is
    /// deterministic, and every field is bound. They cannot prove a wallet
    /// derives the same hash — though the risk is now low, since both sides use
    /// `starknet-core`'s SNIP-12 implementation over the identical document from
    /// [`BindingMessage::typed_data_json`].
    ///
    /// To arm this test: print `typed_data_json()` for the values below, sign
    /// that document with Argent or Braavos, and record the hash the wallet
    /// reports. A mismatch points at the document in `typed_data_json`, not at
    /// the hashing.
    ///
    /// Until armed, verification still fails closed: a mismatch means real
    /// attestations never verify, which is a broken feature and not a hole.
    #[test]
    #[ignore = "needs a real wallet signature; see doc comment"]
    fn matches_wallet_produced_hash() {
        const WALLET_ACCOUNT: &str = "0xREPLACE_ME";
        const WALLET_PUBKEY: &str = "REPLACE_ME_64_HEX_CHARS";
        const WALLET_CHAIN: &str = "SN_SEPOLIA";
        const WALLET_SIGNED_AT: u64 = 0;
        const WALLET_REPORTED_HASH: &str = "0xREPLACE_ME";

        let derived = BindingMessage {
            nostr_pubkey: WALLET_PUBKEY,
            chain_id: WALLET_CHAIN,
            signed_at: WALLET_SIGNED_AT,
            account_address: WALLET_ACCOUNT,
        }
        .message_hash_hex()
        .expect("derive");

        assert_eq!(derived, WALLET_REPORTED_HASH);
    }
}
