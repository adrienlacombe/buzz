//! NIP-SW Starknet wallet bindings — publish and read.
//!
//! See `docs/nips/NIP-SW.md`. Four subcommands, in the order you use them:
//!
//! 1. `message` prints the SNIP-12 document to hand to a wallet for signing.
//! 2. `publish` takes the resulting signature and publishes the binding.
//! 3. `get` reads bindings by author (forward lookup).
//! 4. `lookup` reads bindings by account address (reverse lookup, via `#i`).
//!
//! This module never touches key material. Signing happens in the user's wallet;
//! the CLI only carries the resulting signature felts.

use crate::client::{normalize_write_response, BuzzClient};
use crate::error::CliError;
use buzz_core::kind::KIND_STARKNET_WALLET_BINDING;
use buzz_core::snip12::BindingMessage;
use buzz_core::wallet_binding::{
    i_tag_value, Attestation, AttestationScheme, SignerScheme, WalletBinding,
};

/// Map a signer-scheme flag to the payload enum.
fn parse_signer_scheme(value: &str) -> Result<SignerScheme, CliError> {
    match value {
        "stark" => Ok(SignerScheme::Stark),
        "secp256k1-ecdsa" => Ok(SignerScheme::Secp256k1Ecdsa),
        "secp256r1" => Ok(SignerScheme::Secp256r1),
        other => Err(CliError::Usage(format!(
            "unknown --signer-scheme '{other}': expected stark, secp256k1-ecdsa, or secp256r1"
        ))),
    }
}

/// Print the SNIP-12 document to sign, plus the hash it derives to.
///
/// Takes the pubkey as a string rather than reading it from a client, because
/// this derivation needs only the **public** key. Requiring a private key to
/// produce a document you hand to a wallet would mean the secret has to be
/// present on whatever machine prepares the signing request, for no reason.
/// `run()` routes the `--pubkey` form before the auth gate; without it the
/// caller's own identity is used.
///
/// `signed_at` is echoed because `publish` must be given the *same* value — the
/// relay re-derives the hash from it, so a different timestamp produces a
/// different message and the attestation is rejected.
pub fn cmd_message(
    pubkey: &str,
    address: &str,
    chain: &str,
    signed_at: Option<u64>,
) -> Result<(), CliError> {
    let signed_at = match signed_at {
        Some(value) => value,
        None => std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| CliError::Other(format!("system clock before unix epoch: {e}")))?
            .as_secs(),
    };
    let message = BindingMessage {
        nostr_pubkey: pubkey,
        chain_id: chain,
        signed_at,
        account_address: address,
    };
    let typed_data = message
        .typed_data_json()
        .map_err(|e| CliError::Usage(format!("could not build SNIP-12 document: {e}")))?;
    let message_hash = message
        .message_hash_hex()
        .map_err(|e| CliError::Usage(format!("could not derive message hash: {e}")))?;

    let output = serde_json::json!({
        "nostr_pubkey": pubkey,
        "address": address,
        "chain_id": chain,
        "signed_at": signed_at,
        "message_hash": message_hash,
        "typed_data": typed_data,
        "next": format!(
            "sign typed_data with the account, then: buzz wallet publish --address {address} --chain {chain} --signed-at {signed_at} --signature <felt> [--signature <felt>…]"
        ),
    });
    println!("{}", serde_json::to_string(&output).unwrap_or_default());
    Ok(())
}

/// Publish a binding for the caller's pubkey.
async fn cmd_publish(
    client: &BuzzClient,
    address: &str,
    chain: &str,
    signature: &[String],
    signed_at: u64,
    class_hash: Option<&str>,
    signer_scheme: &str,
) -> Result<(), CliError> {
    if signature.is_empty() {
        return Err(CliError::Usage(
            "at least one --signature felt is required".into(),
        ));
    }
    let binding = WalletBinding {
        address: address.to_string(),
        chain_id: chain.to_string(),
        class_hash: class_hash.map(str::to_string),
        signer_scheme: parse_signer_scheme(signer_scheme)?,
        attestation: Attestation {
            scheme: AttestationScheme::Snip12,
            signature: signature.to_vec(),
            signed_at,
        },
    };
    // Validate locally so a malformed address fails as a usage error (exit 1)
    // rather than a relay rejection (exit 2).
    binding
        .validate()
        .map_err(|e| CliError::Usage(e.to_string()))?;

    let builder = buzz_sdk::build_wallet_binding(&binding)
        .map_err(|e| CliError::Other(format!("build_wallet_binding failed: {e}")))?;
    let event = client.sign_event(builder)?;
    let resp = client.submit_event(event).await?;
    println!("{}", normalize_write_response(&resp));
    Ok(())
}

/// Read bindings authored by a pubkey, optionally scoped to one chain.
async fn cmd_get(
    client: &BuzzClient,
    pubkey: Option<&str>,
    chain: Option<&str>,
) -> Result<(), CliError> {
    let author = match pubkey {
        Some(value) => value.to_string(),
        None => client.keys().public_key().to_hex(),
    };
    let mut filter = serde_json::json!({
        "kinds": [KIND_STARKNET_WALLET_BINDING],
        "authors": [author],
    });
    if let Some(chain) = chain {
        filter["#d"] = serde_json::json!([chain]);
    }
    let raw = client.query(&filter).await?;
    println!("{raw}");
    Ok(())
}

/// Reverse lookup: which identities claim this account address.
///
/// Uses the NIP-39 style `i` tag, which is single-letter and therefore the only
/// part of the binding a relay filter can index.
///
/// Multiple results are not a bug and MUST NOT be collapsed: distinct pubkeys can
/// each publish a binding naming the same account. On a conforming relay every
/// one of them was attested by that account, meaning the account really does
/// acknowledge each identity — that is information for the caller, not noise.
async fn cmd_lookup(client: &BuzzClient, address: &str, chain: &str) -> Result<(), CliError> {
    let filter = serde_json::json!({
        "kinds": [KIND_STARKNET_WALLET_BINDING],
        "#i": [i_tag_value(chain, address)],
    });
    let raw = client.query(&filter).await?;
    println!("{raw}");
    Ok(())
}

pub async fn dispatch(cmd: crate::WalletCmd, client: &BuzzClient) -> Result<(), CliError> {
    use crate::WalletCmd;
    match cmd {
        WalletCmd::Message {
            address,
            chain,
            signed_at,
            pubkey,
        } => {
            // `run()` short-circuits the --pubkey form before auth; reaching here
            // with it set is harmless, so resolve rather than assume.
            let pubkey = match pubkey {
                Some(value) => value,
                None => client.keys().public_key().to_hex(),
            };
            cmd_message(&pubkey, &address, &chain, signed_at)
        }
        WalletCmd::Publish {
            address,
            chain,
            signature,
            signed_at,
            class_hash,
            signer_scheme,
        } => {
            cmd_publish(
                client,
                &address,
                &chain,
                &signature,
                signed_at,
                class_hash.as_deref(),
                &signer_scheme,
            )
            .await
        }
        WalletCmd::Get { pubkey, chain } => {
            cmd_get(client, pubkey.as_deref(), chain.as_deref()).await
        }
        WalletCmd::Lookup { address, chain } => cmd_lookup(client, &address, &chain).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signer_scheme_accepts_the_documented_values() {
        assert_eq!(
            parse_signer_scheme("stark").expect("ok"),
            SignerScheme::Stark
        );
        assert_eq!(
            parse_signer_scheme("secp256k1-ecdsa").expect("ok"),
            SignerScheme::Secp256k1Ecdsa
        );
        assert_eq!(
            parse_signer_scheme("secp256r1").expect("ok"),
            SignerScheme::Secp256r1
        );
    }

    #[test]
    fn signer_scheme_rejects_unknown_as_usage_error() {
        // Usage, not Other: exit code 1 tells a caller it is their input, and
        // `Unknown` must never be reachable from the CLI — it exists only so a
        // future on-chain scheme stays parseable when read back.
        assert!(matches!(
            parse_signer_scheme("future-curve"),
            Err(CliError::Usage(_))
        ));
        assert!(matches!(
            parse_signer_scheme("unknown"),
            Err(CliError::Usage(_))
        ));
    }
}
