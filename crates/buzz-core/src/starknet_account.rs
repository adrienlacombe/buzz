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

    #[test]
    fn rejects_invalid_class_hash() {
        assert!(matches!(
            account_address_from_hex("not-hex", PUBKEY),
            Err(AddressError::InvalidClassHash(_))
        ));
    }
}
