//! Nostr-key-controlled Starknet accounts — derive, declare, deploy.
//!
//! Three subcommands:
//!
//! 1. `class-hash` computes the class hash of a compiled Sierra artifact.
//! 2. `address` derives the account address for a Nostr pubkey.
//! 3. `deploy` deploys the account, which pays its own fee from that address.
//!
//! The account validates BIP-340 Schnorr signatures from the Nostr key itself,
//! so the key IS the account signer. See `contracts/src/account.cairo`.

use crate::client::BuzzClient;
use crate::error::CliError;
use buzz_core::starknet_account::{account_address_from_hex, constructor_calldata, DEPLOY_SALT};

pub fn cmd_class_hash(artifact: &str) -> Result<(), CliError> {
    let json = std::fs::read_to_string(artifact)
        .map_err(|e| CliError::Usage(format!("cannot read artifact '{artifact}': {e}")))?;
    let class: starknet_core::types::contract::SierraClass = serde_json::from_str(&json)
        .map_err(|e| CliError::Usage(format!("not a Sierra contract class: {e}")))?;
    let class_hash = class
        .class_hash()
        .map_err(|e| CliError::Other(format!("class hash computation failed: {e}")))?;
    let output = serde_json::json!({
        "artifact": artifact,
        "class_hash": class_hash.to_fixed_hex_string(),
    });
    println!("{}", serde_json::to_string(&output).unwrap_or_default());
    Ok(())
}

/// Derive the counterfactual Starknet account address for a Nostr pubkey.
///
/// The address exists as a computation before the account exists on chain, so it
/// can be funded first and deployed later out of its own balance. Local only.
pub fn cmd_address(pubkey: &str, class_hash: &str) -> Result<(), CliError> {
    let address =
        account_address_from_hex(class_hash, pubkey).map_err(|e| CliError::Usage(e.to_string()))?;
    let calldata = constructor_calldata(pubkey).map_err(|e| CliError::Usage(e.to_string()))?;
    let output = serde_json::json!({
        "nostr_pubkey": pubkey,
        "class_hash": class_hash,
        "salt": DEPLOY_SALT.to_fixed_hex_string(),
        "deployer_address": "0x0",
        "constructor_calldata": calldata
            .iter()
            .map(starknet_core::types::Felt::to_fixed_hex_string)
            .collect::<Vec<_>>(),
        "address": address.to_fixed_hex_string(),
        "note": "counterfactual: fund this address, then deploy the account from its own balance. Verify against a real deployment before sending anything you cannot lose.",
    });
    println!("{}", serde_json::to_string(&output).unwrap_or_default());
    Ok(())
}

/// Deploy the caller's Nostr-controlled Starknet account.
///
/// A `DEPLOY_ACCOUNT` transaction is paid for by the account being deployed, so
/// the derived address must already hold funds. That is the point of a
/// counterfactual address: fund it, then it deploys itself.
///
/// `--dry-run` estimates instead of sending. Estimation still requires the address
/// to be funded, because the protocol validates the fee against its balance.
async fn cmd_deploy(
    client: &BuzzClient,
    class_hash: &str,
    rpc_url: &str,
    dry_run: bool,
) -> Result<(), CliError> {
    use starknet_accounts::AccountFactory;

    let class_hash = starknet_core::types::Felt::from_hex(class_hash)
        .map_err(|e| CliError::Usage(format!("invalid class hash: {e}")))?;
    let pubkey = client.keys().public_key().to_hex();
    let calldata = constructor_calldata(&pubkey).map_err(|e| CliError::Usage(e.to_string()))?;

    let url =
        url::Url::parse(rpc_url).map_err(|e| CliError::Usage(format!("invalid --rpc url: {e}")))?;
    let provider = starknet_providers::JsonRpcClient::new(
        starknet_providers::jsonrpc::HttpTransport::new(url),
    );
    // Read the chain id from the node rather than taking a flag: a mismatch would
    // produce a signature valid for a different chain, which the sequencer
    // rejects with nothing to point at the cause.
    let chain_id = starknet_providers::Provider::chain_id(&provider)
        .await
        .map_err(|e| CliError::Other(format!("could not read chain id: {e}")))?;

    // Deref: nostr::SecretKey wraps secp256k1::SecretKey.
    let secret_key = **client.keys().secret_key();
    let factory = crate::starknet_factory::NostrAccountFactory::new(
        class_hash, calldata, chain_id, provider, secret_key,
    );

    let deployment = factory.deploy_v3(DEPLOY_SALT);
    let address = deployment.address();
    let derived = account_address_from_hex(&class_hash.to_fixed_hex_string(), &pubkey)
        .map_err(|e| CliError::Usage(e.to_string()))?;

    if dry_run {
        let estimate = deployment
            .estimate_fee()
            .await
            .map_err(|e| CliError::Other(format!("fee estimation failed: {e}")))?;
        let output = serde_json::json!({
            "dry_run": true,
            "address": address.to_fixed_hex_string(),
            "address_matches_local_derivation": address == derived,
            "chain_id": chain_id.to_fixed_hex_string(),
            "overall_fee": estimate.overall_fee.to_string(),
            "l2_gas_consumed": estimate.l2_gas_consumed.to_string(),
        });
        println!("{}", serde_json::to_string(&output).unwrap_or_default());
        return Ok(());
    }

    let sent = deployment
        .send()
        .await
        .map_err(|e| CliError::Other(format!("deployment failed: {e}")))?;
    let output = serde_json::json!({
        "deployed": true,
        "address": sent.contract_address.to_fixed_hex_string(),
        "transaction_hash": sent.transaction_hash.to_fixed_hex_string(),
        // The whole point of the exercise: does the sequencer land where we said?
        "address_matches_local_derivation": sent.contract_address == derived,
        "locally_derived": derived.to_fixed_hex_string(),
    });
    println!("{}", serde_json::to_string(&output).unwrap_or_default());
    Ok(())
}

pub async fn dispatch(cmd: crate::WalletCmd, client: &BuzzClient) -> Result<(), CliError> {
    use crate::WalletCmd;
    match cmd {
        WalletCmd::ClassHash { artifact } => cmd_class_hash(&artifact),
        WalletCmd::Deploy {
            class_hash,
            rpc,
            dry_run,
        } => cmd_deploy(client, &class_hash, &rpc, dry_run).await,
        WalletCmd::Address { pubkey, class_hash } => {
            let pubkey = match pubkey {
                Some(value) => value,
                None => client.keys().public_key().to_hex(),
            };
            cmd_address(&pubkey, &class_hash)
        }
    }
}
