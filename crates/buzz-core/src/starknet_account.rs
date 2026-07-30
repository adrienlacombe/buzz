//! Address derivation for Nostr-key-controlled Starknet accounts.
//!
//! Companion to `contracts/src/account.cairo`. Given a Nostr pubkey and the
//! deployed class hash, computes the account address **before** the account
//! exists on chain. That ordering is the point: Starknet addresses are a hash of
//! the deployment parameters, so an address can be funded first and deployed
//! later out of its own balance (a "counterfactual" account).
//!
//! Pure computation — no I/O, no network, and no secret key. Deriving an address
//! needs only the public key.
//!
//! # This must agree with the contract
//!
//! [`constructor_calldata`] encodes what `NostrAccount`'s constructor reads. The
//! Cairo signature is `constructor(public_key: u256)`, and Starknet serialises a
//! `u256` as `[low, high]` — so the order here is not cosmetic. Get it wrong and
//! the derived address is simply a different address: funds sent there are
//! unreachable, with no error to warn you.

use starknet_core::utils::get_contract_address;
use starknet_crypto::Felt;

/// Deployment salt for Buzz accounts.
///
/// Zero, because the constructor calldata already carries the pubkey and is
/// hashed into the address — so one pubkey yields one address per class without
/// needing the salt to distinguish them. Fixed rather than random so the address
/// stays derivable from the pubkey alone.
pub const DEPLOY_SALT: Felt = Felt::ZERO;

/// Errors from address derivation.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AddressError {
    /// The Nostr pubkey was not 32 bytes of hex.
    #[error("nostr pubkey must be 32 bytes of hex, got {0}")]
    InvalidPubkey(String),
    /// The class hash was not a valid felt.
    #[error("invalid class hash: {0}")]
    InvalidClassHash(String),
}

/// Splits a 32-byte hex Nostr pubkey into the `(low, high)` felts of a `u256`.
///
/// Returned low-first to match Starknet's `u256` calldata order, which is the
/// order [`constructor_calldata`] emits.
pub fn pubkey_felts(nostr_pubkey: &str) -> Result<(Felt, Felt), AddressError> {
    let raw = nostr_pubkey.strip_prefix("0x").unwrap_or(nostr_pubkey);
    let bytes =
        hex::decode(raw).map_err(|_| AddressError::InvalidPubkey("not valid hex".to_string()))?;
    if bytes.len() != 32 {
        return Err(AddressError::InvalidPubkey(format!(
            "{} bytes",
            bytes.len()
        )));
    }
    // A 32-byte value can exceed the felt252 modulus, so it cannot be one felt.
    // Two 128-bit halves always fit.
    let mut high = [0u8; 32];
    let mut low = [0u8; 32];
    high[16..].copy_from_slice(&bytes[..16]);
    low[16..].copy_from_slice(&bytes[16..]);
    Ok((Felt::from_bytes_be(&low), Felt::from_bytes_be(&high)))
}

/// Constructor calldata for `NostrAccount`: the pubkey as a `u256`.
pub fn constructor_calldata(nostr_pubkey: &str) -> Result<Vec<Felt>, AddressError> {
    let (low, high) = pubkey_felts(nostr_pubkey)?;
    Ok(vec![low, high])
}

/// Derives the counterfactual account address for a Nostr pubkey.
///
/// `deployer_address` is zero: that is what a `DEPLOY_ACCOUNT` transaction uses,
/// as opposed to a `deploy` syscall from another contract. Passing anything else
/// would derive an address the protocol will never deploy to.
pub fn account_address(class_hash: Felt, nostr_pubkey: &str) -> Result<Felt, AddressError> {
    let calldata = constructor_calldata(nostr_pubkey)?;
    Ok(get_contract_address(
        DEPLOY_SALT,
        class_hash,
        &calldata,
        Felt::ZERO,
    ))
}

/// Derives the address from a hex class hash.
pub fn account_address_from_hex(
    class_hash_hex: &str,
    nostr_pubkey: &str,
) -> Result<Felt, AddressError> {
    let class_hash = Felt::from_hex(class_hash_hex)
        .map_err(|e| AddressError::InvalidClassHash(e.to_string()))?;
    account_address(class_hash, nostr_pubkey)
}

/// The exact bytes a `NostrAccount` expects to be BIP-340 signed for `tx_hash`.
///
/// Mirrors `tx_hash_bytes` in `contracts/src/account.cairo`: the 32-byte
/// big-endian representation of the felt. The two encodings must agree byte for
/// byte — if they diverge the account rejects every signature, so this function
/// and its Cairo counterpart are one definition in two languages.
#[must_use]
pub fn tx_hash_message(tx_hash: Felt) -> [u8; 32] {
    tx_hash.to_bytes_be()
}

/// Splits a 32-byte big-endian value into `(low, high)` felts.
fn split_be_32(bytes: &[u8]) -> (Felt, Felt) {
    let mut high = [0u8; 32];
    let mut low = [0u8; 32];
    high[16..].copy_from_slice(&bytes[..16]);
    low[16..].copy_from_slice(&bytes[16..]);
    (Felt::from_bytes_be(&low), Felt::from_bytes_be(&high))
}

/// BIP-340 signs a Starknet transaction hash, in `NostrAccount`'s wire layout.
///
/// Returns `[r_low, r_high, s_low, s_high]` — the order `is_valid_bip340` reads,
/// because a `felt252` cannot hold a `u256` and each scalar is split.
///
/// Signing is **deterministic**: no auxiliary randomness, so the same key and
/// hash always yield the same signature. BIP-340 derives its nonce from a tagged
/// hash of the secret key and message, so this is safe — auxiliary randomness
/// only hardens against fault and side-channel attacks, it is not required for
/// validity. Determinism makes signatures reproducible, which is what lets a
/// caller re-derive and compare rather than trust.
///
/// The caller owns the secret key; nothing here stores, logs, or transmits it.
pub fn sign_tx_hash(secret_key: &secp256k1::SecretKey, tx_hash: Felt) -> [Felt; 4] {
    let secp = secp256k1::Secp256k1::new();
    let keypair = secp256k1::Keypair::from_secret_key(&secp, secret_key);
    let message = secp256k1::Message::from_digest(tx_hash_message(tx_hash));
    let signature = secp.sign_schnorr_no_aux_rand(&message, &keypair);
    signature_felts(&signature.serialize())
}

/// Converts a 64-byte BIP-340 signature into the account's four felts.
///
/// A BIP-340 signature is `bytes(R.x) || bytes(s)`, each 32 bytes big-endian.
#[must_use]
pub fn signature_felts(signature: &[u8; 64]) -> [Felt; 4] {
    let (r_low, r_high) = split_be_32(&signature[..32]);
    let (s_low, s_high) = split_be_32(&signature[32..]);
    [r_low, r_high, s_low, s_high]
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBKEY: &str = "8dae5a92916c512029ad1534fcf264e0e2e33ce492acf34588bc6268f7570dd5";
    const CLASS_HASH: &str = "0x0663fc01a0dbe1bacc4cd2a4c856eb9784b255a20988aa33d4d52b6fc20bd024";

    #[test]
    fn pubkey_splits_low_first_matching_u256_calldata_order() {
        let (low, high) = pubkey_felts(PUBKEY).expect("split");
        // high is the leading 16 bytes, low the trailing 16.
        assert!(high
            .to_fixed_hex_string()
            .ends_with("8dae5a92916c512029ad1534fcf264e0"));
        assert!(low
            .to_fixed_hex_string()
            .ends_with("e2e33ce492acf34588bc6268f7570dd5"));
    }

    #[test]
    fn calldata_is_low_then_high() {
        // Starknet serialises u256 as [low, high]. Reversing this derives a
        // different, unreachable address rather than raising an error, so the
        // order is asserted explicitly.
        let calldata = constructor_calldata(PUBKEY).expect("calldata");
        let (low, high) = pubkey_felts(PUBKEY).expect("split");
        assert_eq!(calldata, vec![low, high]);
    }

    #[test]
    fn each_half_fits_in_128_bits() {
        let (low, high) = pubkey_felts(PUBKEY).expect("split");
        let max = Felt::from(u128::MAX);
        assert!(low <= max);
        assert!(high <= max);
    }

    #[test]
    fn rejects_wrong_length_pubkey() {
        assert!(matches!(
            pubkey_felts("abcd"),
            Err(AddressError::InvalidPubkey(_))
        ));
        assert!(matches!(
            pubkey_felts(&"aa".repeat(31)),
            Err(AddressError::InvalidPubkey(_))
        ));
    }

    #[test]
    fn rejects_non_hex_pubkey() {
        assert!(matches!(
            pubkey_felts(&"zz".repeat(32)),
            Err(AddressError::InvalidPubkey(_))
        ));
    }

    #[test]
    fn accepts_a_0x_prefixed_pubkey() {
        let with = pubkey_felts(&format!("0x{PUBKEY}")).expect("prefixed");
        let without = pubkey_felts(PUBKEY).expect("bare");
        assert_eq!(with, without);
    }

    #[test]
    fn address_is_deterministic() {
        let a = account_address_from_hex(CLASS_HASH, PUBKEY).expect("derive");
        let b = account_address_from_hex(CLASS_HASH, PUBKEY).expect("derive");
        assert_eq!(a, b);
    }

    #[test]
    fn address_depends_on_the_pubkey() {
        // Two users must not share an account.
        let mine = account_address_from_hex(CLASS_HASH, PUBKEY).expect("derive");
        let other = format!("0{}", &PUBKEY[1..]);
        let theirs = account_address_from_hex(CLASS_HASH, &other).expect("derive");
        assert_ne!(mine, theirs);
    }

    #[test]
    fn address_depends_on_the_class_hash() {
        // A redeployed contract yields a different address for the same key,
        // which is why the class hash is an explicit input rather than a
        // constant that could silently go stale.
        let a = account_address_from_hex(CLASS_HASH, PUBKEY).expect("derive");
        let b = account_address_from_hex("0x1234", PUBKEY).expect("derive");
        assert_ne!(a, b);
    }

    #[test]
    fn address_is_a_normalised_felt() {
        // get_contract_address masks into the addressable range; a value outside
        // it would be rejected by the sequencer.
        let addr = account_address_from_hex(CLASS_HASH, PUBKEY).expect("derive");
        assert!(addr != Felt::ZERO);
        // 2^251 upper bound on Starknet addresses.
        let bound =
            Felt::from_hex("0x800000000000000000000000000000000000000000000000000000000000000")
                .expect("bound");
        assert!(addr < bound);
    }

    //
    // Signing
    //

    /// Published BIP-340 test-vector 0 secret key. Not anyone's key: it is the
    /// value printed in the Bitcoin BIPs test-vectors file.
    /// https://github.com/bitcoin/bips/blob/master/bip-0340/test-vectors.csv
    const VECTOR_0_SECRET: [u8; 32] = [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 3,
    ];
    /// Vector 0's signature over a 32-zero-byte message: R.x then s.
    const VECTOR_0_R: &str = "e907831f80848d1069a5371b402410364bdf1c5f8307b0084c55f1ce2dca8215";
    const VECTOR_0_S: &str = "25f66a4a85ea8b71e482a74f382d2ce5ebeee8fdb2172f477df4900d310536c0";

    fn vector_0_key() -> secp256k1::SecretKey {
        secp256k1::SecretKey::from_slice(&VECTOR_0_SECRET).expect("published vector key")
    }

    #[test]
    fn tx_hash_message_is_32_bytes_big_endian() {
        // Must equal Cairo's tx_hash_bytes. A divergence makes the account
        // reject every signature.
        assert_eq!(tx_hash_message(Felt::ONE)[31], 1);
        assert_eq!(tx_hash_message(Felt::ONE)[0], 0);
        assert_eq!(tx_hash_message(Felt::ZERO), [0u8; 32]);
    }

    #[test]
    fn signing_reproduces_the_published_bip340_vector() {
        // The strongest available cross-check: signing tx hash 0 with vector 0's
        // key must produce vector 0's signature — the very signature the Cairo
        // verifier accepts in contracts/tests/test_account.cairo. Rust signer and
        // Cairo verifier are therefore proven to agree, not assumed to.
        //
        // Uses aux_rand = 32 zero bytes because that is what the published
        // vector specifies. Production signing takes the no-aux-rand path.
        let secp = secp256k1::Secp256k1::new();
        let keypair = secp256k1::Keypair::from_secret_key(&secp, &vector_0_key());
        let signature = secp.sign_schnorr_with_aux_rand(
            &secp256k1::Message::from_digest(tx_hash_message(Felt::ZERO)),
            &keypair,
            &[0u8; 32],
        );
        let bytes = signature.serialize();
        assert_eq!(hex::encode(&bytes[..32]), VECTOR_0_R);
        assert_eq!(hex::encode(&bytes[32..]), VECTOR_0_S);
    }

    #[test]
    fn signature_felts_match_the_accounts_wire_layout() {
        // [r_low, r_high, s_low, s_high] — reassembling must recover the scalars.
        let r = hex::decode(VECTOR_0_R).expect("r");
        let s = hex::decode(VECTOR_0_S).expect("s");
        let mut raw = [0u8; 64];
        raw[..32].copy_from_slice(&r);
        raw[32..].copy_from_slice(&s);

        let [r_low, r_high, s_low, s_high] = signature_felts(&raw);
        assert!(r_high.to_fixed_hex_string().ends_with(&VECTOR_0_R[..32]));
        assert!(r_low.to_fixed_hex_string().ends_with(&VECTOR_0_R[32..]));
        assert!(s_high.to_fixed_hex_string().ends_with(&VECTOR_0_S[..32]));
        assert!(s_low.to_fixed_hex_string().ends_with(&VECTOR_0_S[32..]));
    }

    #[test]
    fn production_signing_verifies_under_bip340() {
        // Independent check: sign with our function, verify with libsecp256k1.
        let secp = secp256k1::Secp256k1::new();
        let key = vector_0_key();
        let keypair = secp256k1::Keypair::from_secret_key(&secp, &key);
        let tx_hash = Felt::from_hex("0x1234abcd").expect("hash");

        let felts = sign_tx_hash(&key, tx_hash);
        // Rebuild the 64-byte signature from the felts we emit.
        let mut raw = [0u8; 64];
        raw[..16].copy_from_slice(&felts[1].to_bytes_be()[16..]);
        raw[16..32].copy_from_slice(&felts[0].to_bytes_be()[16..]);
        raw[32..48].copy_from_slice(&felts[3].to_bytes_be()[16..]);
        raw[48..].copy_from_slice(&felts[2].to_bytes_be()[16..]);

        let signature = secp256k1::schnorr::Signature::from_slice(&raw).expect("64-byte sig");
        let (xonly, _) = keypair.x_only_public_key();
        secp.verify_schnorr(
            &signature,
            &secp256k1::Message::from_digest(tx_hash_message(tx_hash)),
            &xonly,
        )
        .expect("our own signature must verify");
    }

    #[test]
    fn signing_is_deterministic() {
        let key = vector_0_key();
        let hash = Felt::from_hex("0xfeed").expect("hash");
        assert_eq!(sign_tx_hash(&key, hash), sign_tx_hash(&key, hash));
    }

    #[test]
    fn different_hashes_give_different_signatures() {
        // Otherwise one signature would authorise every transaction.
        let key = vector_0_key();
        let a = sign_tx_hash(&key, Felt::from_hex("0x1").expect("a"));
        let b = sign_tx_hash(&key, Felt::from_hex("0x2").expect("b"));
        assert_ne!(a, b);
    }

    #[test]
    fn rejects_invalid_class_hash() {
        assert!(matches!(
            account_address_from_hex("not-hex", PUBKEY),
            Err(AddressError::InvalidClassHash(_))
        ));
    }
}
