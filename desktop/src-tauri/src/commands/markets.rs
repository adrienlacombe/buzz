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
    assert_fee_is_first_call, assert_markets_signing_keyring, betting_halted,
    betting_halted_by_remaining_blocks, build_validated_bet_batch, markets_signing_keyring_name,
    resolve_avnu_proxy_url, resolve_indexer_url, BetCallHex, NOSTR_ACCOUNT_CLASS_HASH,
};
use buzz_core_pkg::outside_execution::{
    any_caller, felt_from_hex, selector_from_name, Felt, OutsideCall, OutsideExecution,
};
use buzz_core_pkg::starknet_account::{
    account_address_from_hex, constructor_calldata, pubkey_felts, sign_tx_hash, DEPLOY_SALT,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
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

/// Live wallet-owned halt signal from mempool.space (not the indexer).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DifficultyHaltStatus {
    /// Blocks remaining until the next difficulty retarget.
    pub remaining_blocks: u64,
    /// Next retarget height from mempool.space when present.
    pub next_retarget_height: Option<u64>,
    /// `true` when `remaining_blocks <= 24`.
    pub halted: bool,
    /// Source used: `mempool` (live) or `height_fallback` (2016-block math).
    pub source: String,
}

const MEMPOOL_DIFFICULTY_ADJUSTMENT_URL: &str =
    "https://mempool.space/api/v1/difficulty-adjustment";
const MEMPOOL_TIP_HEIGHT_URL: &str = "https://mempool.space/api/blocks/tip/height";

/// Fetch the product halt signal. Prefers mempool `remainingBlocks`; falls back
/// to tip-height 2016-block math only if the adjustment endpoint is unavailable.
async fn fetch_difficulty_halt_status() -> Result<DifficultyHaltStatus, String> {
    let client = reqwest::Client::new();
    match client.get(MEMPOOL_DIFFICULTY_ADJUSTMENT_URL).send().await {
        Ok(resp) if resp.status().is_success() => {
            let value: Value = resp
                .json()
                .await
                .map_err(|e| format!("difficulty-adjustment JSON: {e}"))?;
            let remaining = value
                .get("remainingBlocks")
                .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))
                .ok_or_else(|| format!("difficulty-adjustment missing remainingBlocks: {value}"))?;
            let next = value
                .get("nextRetargetHeight")
                .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)));
            Ok(DifficultyHaltStatus {
                remaining_blocks: remaining,
                next_retarget_height: next,
                halted: betting_halted_by_remaining_blocks(remaining),
                source: "mempool".into(),
            })
        }
        Ok(resp) => Err(format!("difficulty-adjustment HTTP {}", resp.status())),
        Err(primary) => {
            // Fallback: tip height + 2016-block math (still wallet-fetched).
            let tip_resp = client
                .get(MEMPOOL_TIP_HEIGHT_URL)
                .send()
                .await
                .map_err(|e| {
                    format!("difficulty-adjustment failed ({primary}); tip height also failed: {e}")
                })?;
            if !tip_resp.status().is_success() {
                return Err(format!(
                    "difficulty-adjustment failed ({primary}); tip height HTTP {}",
                    tip_resp.status()
                ));
            }
            let tip_text = tip_resp
                .text()
                .await
                .map_err(|e| format!("tip height body: {e}"))?;
            let tip: u64 = tip_text
                .trim()
                .parse()
                .map_err(|_| format!("invalid tip height {tip_text:?}"))?;
            let next = buzz_core_pkg::markets::next_retarget_height(tip);
            let remaining = next.saturating_sub(tip);
            Ok(DifficultyHaltStatus {
                remaining_blocks: remaining,
                next_retarget_height: Some(next),
                halted: betting_halted(tip),
                source: "height_fallback".into(),
            })
        }
    }
}

fn felt_hex(v: &str) -> Result<Felt, String> {
    felt_from_hex(v).map_err(|e| e.to_string())
}

fn parse_call(call: &PreparedCall) -> Result<OutsideCall, String> {
    let to = felt_hex(&call.contract_address)?;
    let selector = selector_from_name(&call.entrypoint).map_err(|e| e.to_string())?;
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
    // Pass the real keyring slot name for the keys we are about to use.
    // Tests must exercise this gate with actual `agent:<pubkey>` inputs — see
    // `agent_keyring_slot_used_by_secret_store_is_rejected`.
    let keyring_name = markets_signing_keyring_name();
    assert_markets_signing_keyring(keyring_name).map_err(|e| e.to_string())?;
    state.signing_keys()
}

fn avnu_proxy_url() -> Result<String, String> {
    resolve_avnu_proxy_url().map_err(|e| e.to_string())
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
    // `avnu_proxy_url` already refuses loopback; non-loopback /rpc requires Bearer.
    let url = format!("{}/rpc", avnu_proxy_url()?);
    let token = std::env::var("AVNU_PROXY_AUTH_TOKEN")
        .map(|t| t.trim().to_string())
        .ok()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            "AVNU_PROXY_AUTH_TOKEN is required for non-loopback proxy /rpc \
             (runtime-only from process env; never bake into the client)"
                .to_string()
        })?;
    let client = reqwest::Client::new();
    let req = client.post(&url).json(&body).bearer_auth(token);
    let resp = req
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

/// Wallet-owned halt status for the Markets UI (`remainingBlocks` from
/// mempool.space). Same fetch `place_bet` uses before signing.
#[tauri::command]
pub async fn difficulty_halt_status() -> Result<DifficultyHaltStatus, String> {
    fetch_difficulty_halt_status().await
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
        indexer_url: resolve_indexer_url().map_err(|e| e.to_string())?,
    })
}

/// Bet-screen command: sign + submit prepared calls via AVNU (no Lightning).
///
/// Owns the halt check via mempool.space `remainingBlocks` (not a JS-supplied
/// height). Refuses when `remainingBlocks <= 24`.
///
/// Does **not** sign an arbitrary frontend `Call[]`. Rust rebuilds the wallet
/// fee transfer and accepts only `approve` + `execute_trade` against the
/// product contracts. The fee transfer is always first in the signed batch.
#[tauri::command]
pub async fn place_bet(
    state: State<'_, AppState>,
    calls: Vec<PreparedCall>,
    token_amount: String,
) -> Result<PlaceBetResult, String> {
    let halt = fetch_difficulty_halt_status().await?;
    if halt.halted {
        return Err(format!(
            "Betting is paused until after the next Bitcoin difficulty retarget ({} blocks remaining)",
            halt.remaining_blocks
        ));
    }

    let token_amount: u128 = token_amount
        .parse()
        .map_err(|_| "invalid tokenAmount".to_string())?;

    let incoming: Vec<BetCallHex> = calls
        .iter()
        .map(|c| BetCallHex {
            contract_address: c.contract_address.clone(),
            entrypoint: c.entrypoint.clone(),
            calldata: c.calldata.clone(),
        })
        .collect();
    let (validated, fee_amount) =
        build_validated_bet_batch(&incoming, token_amount).map_err(|e| e.to_string())?;
    // Defense in depth: fee must be first before we talk to AVNU.
    assert_fee_is_first_call(
        &validated[0].contract_address,
        &validated[0].entrypoint,
        &validated[0].calldata,
        fee_amount,
    )
    .map_err(|e| e.to_string())?;

    let keys = human_keys(&state)?;
    let pubkey_hex = keys.public_key().to_hex();
    let account = account_address_from_hex(NOSTR_ACCOUNT_CLASS_HASH, &pubkey_hex)
        .map_err(|e| e.to_string())?;
    let (pk_low, pk_high) = pubkey_felts(&pubkey_hex).map_err(|e| e.to_string())?;

    let rpc_calls: Vec<Value> = validated
        .iter()
        .map(|c| {
            let prepared = PreparedCall {
                contract_address: c.contract_address.clone(),
                entrypoint: c.entrypoint.clone(),
                calldata: c.calldata.clone(),
            };
            // Touch parse early so bad calldata fails before paying the proxy.
            let _ = parse_call(&prepared)?;
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
    // Enforce fee-first on the batch AVNU asked us to sign — not only the
    // rebuilt rpc_calls / PlaceBetResult.fee_amount.
    let first = execution
        .calls
        .first()
        .ok_or_else(|| "typed_data Calls empty".to_string())?;
    let first_to = first.to.to_fixed_hex_string();
    let transfer_selector = selector_from_name("transfer").map_err(|e| e.to_string())?;
    if first.selector != transfer_selector {
        return Err("signed batch must start with wallet fee transfer to FEE_RECIPIENT".into());
    }
    let first_calldata: Vec<String> = first
        .calldata
        .iter()
        .map(|f| f.to_fixed_hex_string())
        .collect();
    assert_fee_is_first_call(&first_to, "transfer", &first_calldata, fee_amount)
        .map_err(|e| e.to_string())?;

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

/// Runtime indexer base URL (`INDEXER_URL` or product host). Honored by the
/// Vite client via this command so documented `INDEXER_URL` does not no-op.
#[tauri::command]
pub async fn markets_indexer_url() -> Result<String, String> {
    resolve_indexer_url().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_core_pkg::markets::{
        assert_markets_signing_keyring, is_human_keyring_name, MarketsError,
        HUMAN_IDENTITY_KEYRING_NAME,
    };

    #[test]
    fn agent_keyring_slot_used_by_secret_store_is_rejected() {
        // Real naming from secret_store / managed agents — not only the
        // HUMAN_IDENTITY_KEYRING_NAME constant (which always Ok's).
        let agent = "agent:8dae5a92916c512029ad1534fcf264e0e2e33ce492acf34588bc6268f7570dd5";
        assert_eq!(
            assert_markets_signing_keyring(agent),
            Err(MarketsError::AgentKeyNotAllowed)
        );
        assert_eq!(
            assert_markets_signing_keyring("agent:abc123"),
            Err(MarketsError::AgentKeyNotAllowed)
        );
        assert!(is_human_keyring_name(HUMAN_IDENTITY_KEYRING_NAME));
        assert_eq!(
            assert_markets_signing_keyring(markets_signing_keyring_name()),
            Ok(())
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
