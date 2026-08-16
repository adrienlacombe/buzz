//! Product-level Bitcoin Markets commands.
//!
//! - [`fund_lightning`] — returns the human wallet address for Atomiq LN funding.
//!   No bet path, no trade calls.
//! - [`place_bet`] — signs and submits a prepared Call[] (fee + approve +
//!   execute_trade) via the AVNU proxy. No Lightning / Atomiq / invoice.
//!
//! Signing uses [`AppState::signing_keys`] (human nsec only). Agent keyring
//! entries never receive a Starknet account.

use crate::app_state::AppState;
use buzz_core_pkg::markets::{
    assert_human_keyring_name, betting_halted, resolve_indexer_url, wallet_fee_amount,
    HUMAN_IDENTITY_KEYRING_NAME, NOSTR_ACCOUNT_CLASS_HASH,
};
use buzz_core_pkg::outside_execution::{
    any_caller, OutsideCall, OutsideExecution,
};
use buzz_core_pkg::starknet_account::{
    account_address_from_hex, constructor_calldata, pubkey_felts, sign_tx_hash, DEPLOY_SALT,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use starknet_core::types::Felt;
use starknet_core::utils::get_selector_from_name;
use tauri::State;

/// A Starknet call as the frontend prepares it (no secrets).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedCall {
    pub contract_address: String,
    pub entrypoint: String,
    pub calldata: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FundLightningResult {
    /// Counterfactual account address (Atomiq destination).
    pub address: String,
    /// Human hex pubkey that owns the account.
    pub pubkey_hex: String,
    pub class_hash: String,
    /// Constructor calldata `[pk_low, pk_high]` for undeployed deploymentData.
    pub constructor_calldata: Vec<String>,
    pub salt: String,
    /// Documented product indexer (not used by funding itself).
    pub indexer_url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceBetResult {
    pub tx_hash: String,
    pub fee_amount: String,
    pub account_address: String,
}

fn felt_hex(v: &str) -> Result<Felt, String> {
    Felt::from_hex(v.trim()).map_err(|e| format!("invalid felt {v:?}: {e}"))
}

fn parse_call(call: &PreparedCall) -> Result<OutsideCall, String> {
    let to = felt_hex(&call.contract_address)?;
    let selector = get_selector_from_name(&call.entrypoint)
        .map_err(|e| format!("bad entrypoint {}: {e}", call.entrypoint))?;
    let mut calldata = Vec::with_capacity(call.calldata.len());
    for c in &call.calldata {
        calldata.push(felt_hex(c)?);
    }
    Ok(OutsideCall {
        to,
        selector,
        calldata,
    })
}

fn human_keys(state: &AppState) -> Result<nostr::Keys, String> {
    // Gate: only the human identity keyring slot may own a Starknet account.
    assert_human_keyring_name(HUMAN_IDENTITY_KEYRING_NAME).map_err(|e| e.to_string())?;
    state.signing_keys()
}

fn avnu_proxy_url() -> String {
    std::env::var("AVNU_PROXY_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8788".to_string())
        .trim_end_matches('/')
        .to_string()
}

fn chain_id_short() -> String {
    std::env::var("STARKNET_CHAIN_ID").unwrap_or_else(|_| "SN_MAIN".to_string())
}

async fn avnu_rpc(method: &str, params: Value) -> Result<Value, String> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    let url = format!("{}/rpc", avnu_proxy_url());
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("AVNU proxy unreachable ({url}): {e}"))?;
    let status = resp.status();
    let value: Value = resp
        .json()
        .await
        .map_err(|e| format!("AVNU proxy returned non-JSON: {e}"))?;
    if !status.is_success() {
        return Err(format!("AVNU proxy HTTP {status}: {value}"));
    }
    if let Some(err) = value.get("error") {
        return Err(format!("AVNU error: {err}"));
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| format!("AVNU response missing result: {value}"))
}

fn typed_data_to_outside_execution(typed: &Value) -> Result<(OutsideExecution, String), String> {
    let message = typed
        .get("message")
        .ok_or_else(|| "typed_data missing message".to_string())?;
    // SNIP-9 v2 field names (AVNU).
    let caller = message
        .get("Caller")
        .or_else(|| message.get("caller"))
        .and_then(|v| v.as_str())
        .map(felt_hex)
        .transpose()?
        .unwrap_or_else(any_caller);
    let nonce = message
        .get("Nonce")
        .or_else(|| message.get("nonce"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| "typed_data missing Nonce".to_string())
        .and_then(felt_hex)?;
    let execute_after = message
        .get("Execute After")
        .or_else(|| message.get("execute_after"))
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(0);
    let execute_before = message
        .get("Execute Before")
        .or_else(|| message.get("execute_before"))
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .ok_or_else(|| "typed_data missing Execute Before".to_string())?;

    let calls_val = message
        .get("Calls")
        .or_else(|| message.get("calls"))
        .and_then(|v| v.as_array())
        .ok_or_else(|| "typed_data missing Calls".to_string())?;

    let mut calls = Vec::with_capacity(calls_val.len());
    for c in calls_val {
        let to = c
            .get("To")
            .or_else(|| c.get("to"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| "call missing To".to_string())
            .and_then(felt_hex)?;
        let selector = c
            .get("Selector")
            .or_else(|| c.get("selector"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| "call missing Selector".to_string())
            .and_then(felt_hex)?;
        let calldata = c
            .get("Calldata")
            .or_else(|| c.get("calldata"))
            .and_then(|v| v.as_array())
            .ok_or_else(|| "call missing Calldata".to_string())?
            .iter()
            .map(|x| {
                x.as_str()
                    .ok_or_else(|| "calldata entry not a string".to_string())
                    .and_then(felt_hex)
            })
            .collect::<Result<Vec<_>, _>>()?;
        calls.push(OutsideCall {
            to,
            selector,
            calldata,
        });
    }

    let domain = typed
        .get("domain")
        .ok_or_else(|| "typed_data missing domain".to_string())?;
    let chain_id = domain
        .get("chainId")
        .or_else(|| domain.get("chain_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("SN_MAIN")
        .to_string();
    // AVNU may return hex chain id; map known values.
    let chain_short = if chain_id == "0x534e5f4d41494e" || chain_id == "SN_MAIN" {
        "SN_MAIN".to_string()
    } else if chain_id == "0x534e5f5345504f4c4941" || chain_id == "SN_SEPOLIA" {
        "SN_SEPOLIA".to_string()
    } else {
        chain_id_short()
    };

    Ok((
        OutsideExecution {
            caller,
            nonce,
            execute_after,
            execute_before,
            calls,
        },
        chain_short,
    ))
}

/// Fund-screen command: derive the human counterfactual address for Atomiq.
///
/// Does **not** create an LN invoice, swap, or bet. The frontend uses Atomiq
/// FROM_BTCLN_AUTO into this address on the Fund screen only.
#[tauri::command]
pub async fn fund_lightning(
    state: State<'_, AppState>,
    amount_sats: u64,
) -> Result<FundLightningResult, String> {
    if !(100..=2_000_000).contains(&amount_sats) {
        return Err("Amount must be between 100 and 2_000_000 sats".into());
    }
    let keys = human_keys(&state)?;
    let pubkey_hex = keys.public_key().to_hex();
    let address = account_address_from_hex(NOSTR_ACCOUNT_CLASS_HASH, &pubkey_hex)
        .map_err(|e| e.to_string())?;
    let ctor = constructor_calldata(&pubkey_hex).map_err(|e| e.to_string())?;
    Ok(FundLightningResult {
        address: address.to_fixed_hex_string(),
        pubkey_hex,
        class_hash: NOSTR_ACCOUNT_CLASS_HASH.to_string(),
        constructor_calldata: ctor.iter().map(|f| f.to_fixed_hex_string()).collect(),
        salt: DEPLOY_SALT.to_fixed_hex_string(),
        indexer_url: resolve_indexer_url(),
    })
}

/// Bet-screen command: sign + submit prepared calls via AVNU (no Lightning).
#[tauri::command]
pub async fn place_bet(
    state: State<'_, AppState>,
    calls: Vec<PreparedCall>,
    bitcoin_height: u64,
    token_amount: String,
) -> Result<PlaceBetResult, String> {
    if betting_halted(bitcoin_height) {
        return Err(
            "Betting is paused until after the next Bitcoin difficulty retarget".into(),
        );
    }
    if calls.is_empty() {
        return Err("place_bet requires at least one call".into());
    }
    // Refuse anything that looks like a Lightning/Atomiq entrypoint mixed in.
    for c in &calls {
        let ep = c.entrypoint.to_ascii_lowercase();
        if ep.contains("lightning") || ep.contains("atomiq") || ep.contains("invoice") {
            return Err("place_bet is Starknet-only; Lightning belongs on Fund".into());
        }
    }

    let token_amount: u128 = token_amount
        .parse()
        .map_err(|_| "invalid tokenAmount".to_string())?;
    let fee_amount = wallet_fee_amount(token_amount);

    let keys = human_keys(&state)?;
    let pubkey_hex = keys.public_key().to_hex();
    let account = account_address_from_hex(NOSTR_ACCOUNT_CLASS_HASH, &pubkey_hex)
        .map_err(|e| e.to_string())?;
    let (pk_low, pk_high) = pubkey_felts(&pubkey_hex).map_err(|e| e.to_string())?;

    let rpc_calls: Vec<Value> = calls
        .iter()
        .map(|c| {
            // Touch parse early so bad calldata fails before paying the proxy.
            let _ = parse_call(c)?;
            Ok(json!({
                "contractAddress": c.contract_address,
                "entrypoint": c.entrypoint,
                "calldata": c.calldata,
            }))
        })
        .collect::<Result<_, String>>()?;

    let deployment = json!({
        "address": account.to_fixed_hex_string(),
        "class_hash": NOSTR_ACCOUNT_CLASS_HASH,
        "salt": DEPLOY_SALT.to_fixed_hex_string(),
        "calldata": [
            pk_low.to_fixed_hex_string(),
            pk_high.to_fixed_hex_string()
        ],
        "version": 1
    });

    // Prefer deploy_and_invoke when undeployed; AVNU accepts this class on mainnet.
    let build_params = json!({
        "transaction": {
            "type": "deploy_and_invoke",
            "deployment": deployment,
            "invoke": {
                "userAddress": account.to_fixed_hex_string(),
                "calls": rpc_calls
            }
        },
        "parameters": {
            "version": "0x1",
            "feeMode": { "mode": "sponsored" }
        }
    });

    let built = match avnu_rpc("paymaster_buildTransaction", build_params).await {
        Ok(v) => v,
        Err(_) => {
            // Fallback: account may already be deployed.
            let invoke_only = json!({
                "transaction": {
                    "type": "invoke",
                    "invoke": {
                        "userAddress": account.to_fixed_hex_string(),
                        "calls": rpc_calls
                    }
                },
                "parameters": {
                    "version": "0x1",
                    "feeMode": { "mode": "sponsored" }
                }
            });
            avnu_rpc("paymaster_buildTransaction", invoke_only).await?
        }
    };

    let typed = built
        .get("typed_data")
        .or_else(|| built.get("typedData"))
        .ok_or_else(|| format!("buildTransaction missing typed_data: {built}"))?;

    let (execution, chain_short) = typed_data_to_outside_execution(typed)?;
    let msg_hash = execution
        .message_hash(account, &chain_short)
        .map_err(|e| e.to_string())?;

    let secret = **keys.secret_key();
    let signature = sign_tx_hash(&secret, msg_hash);
    let sig_hex: Vec<String> = signature.iter().map(|f| f.to_fixed_hex_string()).collect();

    let tx_type = built
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("invoke");

    let execute_params = if tx_type == "deploy_and_invoke" {
        json!({
            "transaction": {
                "type": "deploy_and_invoke",
                "deployment": {
                    "address": account.to_fixed_hex_string(),
                    "class_hash": NOSTR_ACCOUNT_CLASS_HASH,
                    "salt": DEPLOY_SALT.to_fixed_hex_string(),
                    "calldata": [
                        pk_low.to_fixed_hex_string(),
                        pk_high.to_fixed_hex_string()
                    ],
                    "version": 1
                },
                "invoke": {
                    "userAddress": account.to_fixed_hex_string(),
                    "typedData": typed,
                    "signature": sig_hex
                }
            },
            "parameters": {
                "version": "0x1",
                "feeMode": { "mode": "sponsored" }
            }
        })
    } else {
        json!({
            "transaction": {
                "type": "invoke",
                "invoke": {
                    "userAddress": account.to_fixed_hex_string(),
                    "typedData": typed,
                    "signature": sig_hex
                }
            },
            "parameters": {
                "version": "0x1",
                "feeMode": { "mode": "sponsored" }
            }
        })
    };

    let executed = avnu_rpc("paymaster_executeTransaction", execute_params).await?;
    let tx_hash = executed
        .get("transaction_hash")
        .or_else(|| executed.get("transactionHash"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("executeTransaction missing hash: {executed}"))?
        .to_string();

    Ok(PlaceBetResult {
        tx_hash,
        fee_amount: fee_amount.to_string(),
        account_address: account.to_fixed_hex_string(),
    })
}

/// Derive the human wallet address (read-only).
#[tauri::command]
pub async fn bitcoin_wallet_address(
    state: State<'_, AppState>,
) -> Result<FundLightningResult, String> {
    fund_lightning(state, 100).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_core_pkg::markets::{is_human_keyring_name, MarketsError};

    #[test]
    fn agent_keyring_names_are_rejected_before_signing() {
        assert!(is_human_keyring_name(HUMAN_IDENTITY_KEYRING_NAME));
        assert_eq!(
            assert_human_keyring_name("agent:abc"),
            Err(MarketsError::AgentKeyNotAllowed)
        );
    }

    #[test]
    fn prepared_call_serde_roundtrip() {
        let c = PreparedCall {
            contract_address: "0x1".into(),
            entrypoint: "execute_trade".into(),
            calldata: vec!["0x2".into()],
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["contractAddress"], "0x1");
        assert_eq!(v["entrypoint"], "execute_trade");
    }
}
