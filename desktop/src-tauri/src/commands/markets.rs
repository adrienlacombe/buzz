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
    assert_fee_is_first_call, assert_markets_signing_keyring, betting_halted_by_remaining_blocks,
    build_validated_bet_batch, markets_signing_keyring_name, resolve_avnu_proxy_url,
    resolve_indexer_url, BetCallHex, NOSTR_ACCOUNT_CLASS_HASH,
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
    /// Always `mempool` — product path has no tip-height fallback.
    pub source: String,
}

const MEMPOOL_DIFFICULTY_ADJUSTMENT_URL: &str =
    "https://mempool.space/api/v1/difficulty-adjustment";

/// Parse mempool difficulty-adjustment JSON into a halt status.
///
/// Fail closed: missing/`null` `remainingBlocks` is an error (never "not halted").
fn difficulty_halt_status_from_adjustment(value: &Value) -> Result<DifficultyHaltStatus, String> {
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

/// Fetch the product halt signal from mempool `remainingBlocks` only.
///
/// No tip-height / 2016-block fallback: if the adjustment endpoint is down or
/// the field is missing, return an error so `place_bet` aborts without signing.
async fn fetch_difficulty_halt_status() -> Result<DifficultyHaltStatus, String> {
    let client = reqwest::Client::new();
    let resp = client
        .get(MEMPOOL_DIFFICULTY_ADJUSTMENT_URL)
        .send()
        .await
        .map_err(|e| format!("difficulty-adjustment unreachable: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("difficulty-adjustment HTTP {}", resp.status()));
    }
    let value: Value = resp
        .json()
        .await
        .map_err(|e| format!("difficulty-adjustment JSON: {e}"))?;
    difficulty_halt_status_from_adjustment(&value)
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

/// SNIP-29 call as JSON-RPC over the AVNU proxy (snake_case wire names).
///
/// starknet.js uses camelCase (`contractAddress` / `entrypoint` / `userAddress`);
/// the hosted paymaster schema expects `to` / `selector` / `user_address`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct Snip29Call {
    to: String,
    selector: String,
    calldata: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct Snip29FeeMode {
    mode: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct Snip29Parameters {
    version: String,
    fee_mode: Snip29FeeMode,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct Snip29Deployment {
    address: String,
    class_hash: String,
    salt: String,
    calldata: Vec<String>,
    version: u8,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct Snip29InvokeBuild {
    user_address: String,
    calls: Vec<Snip29Call>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct Snip29InvokeExecute {
    user_address: String,
    typed_data: Value,
    signature: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Snip29TransactionBuild {
    DeployAndInvoke {
        deployment: Snip29Deployment,
        invoke: Snip29InvokeBuild,
    },
    Invoke {
        invoke: Snip29InvokeBuild,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Snip29TransactionExecute {
    DeployAndInvoke {
        deployment: Snip29Deployment,
        invoke: Snip29InvokeExecute,
    },
    Invoke {
        invoke: Snip29InvokeExecute,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct Snip29BuildParams {
    transaction: Snip29TransactionBuild,
    parameters: Snip29Parameters,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct Snip29ExecuteParams {
    transaction: Snip29TransactionExecute,
    parameters: Snip29Parameters,
}

fn sponsored_parameters() -> Snip29Parameters {
    Snip29Parameters {
        version: "0x1".into(),
        fee_mode: Snip29FeeMode {
            mode: "sponsored".into(),
        },
    }
}

fn snip29_call(call: &PreparedCall) -> Result<Snip29Call, String> {
    let outside = parse_call(call)?;
    Ok(Snip29Call {
        to: outside.to.to_fixed_hex_string(),
        selector: outside.selector.to_fixed_hex_string(),
        calldata: outside
            .calldata
            .iter()
            .map(|f| f.to_fixed_hex_string())
            .collect(),
    })
}

fn snip29_deployment(account: &Felt, pk_low: &Felt, pk_high: &Felt) -> Snip29Deployment {
    Snip29Deployment {
        address: account.to_fixed_hex_string(),
        class_hash: NOSTR_ACCOUNT_CLASS_HASH.to_string(),
        salt: DEPLOY_SALT.to_fixed_hex_string(),
        calldata: vec![pk_low.to_fixed_hex_string(), pk_high.to_fixed_hex_string()],
        version: 1,
    }
}

fn paymaster_build_deploy_and_invoke(
    account: &Felt,
    pk_low: &Felt,
    pk_high: &Felt,
    calls: Vec<Snip29Call>,
) -> Snip29BuildParams {
    Snip29BuildParams {
        transaction: Snip29TransactionBuild::DeployAndInvoke {
            deployment: snip29_deployment(account, pk_low, pk_high),
            invoke: Snip29InvokeBuild {
                user_address: account.to_fixed_hex_string(),
                calls,
            },
        },
        parameters: sponsored_parameters(),
    }
}

fn paymaster_build_invoke(account: &Felt, calls: Vec<Snip29Call>) -> Snip29BuildParams {
    Snip29BuildParams {
        transaction: Snip29TransactionBuild::Invoke {
            invoke: Snip29InvokeBuild {
                user_address: account.to_fixed_hex_string(),
                calls,
            },
        },
        parameters: sponsored_parameters(),
    }
}

fn paymaster_execute_deploy_and_invoke(
    account: &Felt,
    pk_low: &Felt,
    pk_high: &Felt,
    typed_data: Value,
    signature: Vec<String>,
) -> Snip29ExecuteParams {
    Snip29ExecuteParams {
        transaction: Snip29TransactionExecute::DeployAndInvoke {
            deployment: snip29_deployment(account, pk_low, pk_high),
            invoke: Snip29InvokeExecute {
                user_address: account.to_fixed_hex_string(),
                typed_data,
                signature,
            },
        },
        parameters: sponsored_parameters(),
    }
}

fn paymaster_execute_invoke(
    account: &Felt,
    typed_data: Value,
    signature: Vec<String>,
) -> Snip29ExecuteParams {
    Snip29ExecuteParams {
        transaction: Snip29TransactionExecute::Invoke {
            invoke: Snip29InvokeExecute {
                user_address: account.to_fixed_hex_string(),
                typed_data,
                signature,
            },
        },
        parameters: sponsored_parameters(),
    }
}

fn snip29_params_to_value<T: Serialize>(params: &T) -> Result<Value, String> {
    serde_json::to_value(params).map_err(|e| format!("SNIP-29 serialize: {e}"))
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
    // Fail closed: unreachable/malformed adjustment → Err, no signing.
    let halt = fetch_difficulty_halt_status()
        .await
        .map_err(|e| format!("Betting is unavailable (Bitcoin height source unreachable): {e}"))?;
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

    let rpc_calls: Vec<Snip29Call> = validated
        .iter()
        .map(|c| {
            let prepared = PreparedCall {
                contract_address: c.contract_address.clone(),
                entrypoint: c.entrypoint.clone(),
                calldata: c.calldata.clone(),
            };
            // Touch parse early so bad calldata fails before paying the proxy.
            snip29_call(&prepared)
        })
        .collect::<Result<_, String>>()?;

    // Prefer deploy_and_invoke when undeployed; AVNU accepts this class on mainnet.
    // Wire format is SNIP-29 snake_case via serde — never hand-build camelCase JSON.
    let build_params = snip29_params_to_value(&paymaster_build_deploy_and_invoke(
        &account,
        &pk_low,
        &pk_high,
        rpc_calls.clone(),
    ))?;

    let built = match avnu_rpc("paymaster_buildTransaction", build_params).await {
        Ok(v) => v,
        Err(_) => {
            // Fallback: account may already be deployed.
            let invoke_only = snip29_params_to_value(&paymaster_build_invoke(&account, rpc_calls))?;
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
        snip29_params_to_value(&paymaster_execute_deploy_and_invoke(
            &account,
            &pk_low,
            &pk_high,
            typed.clone(),
            sig_hex,
        ))?
    } else {
        snip29_params_to_value(&paymaster_execute_invoke(&account, typed.clone(), sig_hex))?
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
        assert_markets_signing_keyring, betting_halted_by_remaining_blocks, is_human_keyring_name,
        MarketsError, HUMAN_IDENTITY_KEYRING_NAME,
    };
    use serde_json::json;

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

    #[test]
    fn remaining_blocks_signal_halts_at_24() {
        // Import helper — do not reimplement the threshold.
        assert!(!betting_halted_by_remaining_blocks(25));
        assert!(betting_halted_by_remaining_blocks(24));
        assert!(betting_halted_by_remaining_blocks(0));
        let open = difficulty_halt_status_from_adjustment(&json!({
            "remainingBlocks": 25,
            "nextRetargetHeight": 963648,
        }))
        .expect("25 remaining must parse");
        assert!(!open.halted);
        assert_eq!(open.remaining_blocks, 25);
        assert_eq!(open.source, "mempool");
        let halted = difficulty_halt_status_from_adjustment(&json!({
            "remainingBlocks": 24,
        }))
        .expect("24 remaining must parse");
        assert!(halted.halted);
        let at_retarget = difficulty_halt_status_from_adjustment(&json!({
            "remainingBlocks": 0,
        }))
        .expect("0 remaining must parse");
        assert!(at_retarget.halted);
    }

    #[test]
    fn missing_adjustment_remaining_blocks_fails_closed() {
        // Product hole this PR closes: never treat a bad/missing adjustment
        // payload as "not halted" (no tip-height green light).
        let err = difficulty_halt_status_from_adjustment(&json!({
            "nextRetargetHeight": 963648,
        }))
        .expect_err("missing remainingBlocks must error");
        assert!(
            err.contains("remainingBlocks"),
            "error must name the field: {err}"
        );
        let null_err = difficulty_halt_status_from_adjustment(&json!({
            "remainingBlocks": null,
        }))
        .expect_err("null remainingBlocks must error");
        assert!(null_err.contains("remainingBlocks"));
        assert!(difficulty_halt_status_from_adjustment(&json!({})).is_err());
    }

    #[test]
    fn snip29_paymaster_params_use_snake_case_wire_names() {
        // Live AVNU / buzz-avnu-proxy reject starknet.js camelCase
        // (userAddress / contractAddress / feeMode) with -32602.
        let account = Felt::from_hex_unchecked("0x1234");
        let pk_low = Felt::from_hex_unchecked("0x1");
        let pk_high = Felt::from_hex_unchecked("0x2");
        let calls = vec![Snip29Call {
            to: "0xabc".into(),
            selector: "0xdef".into(),
            calldata: vec!["0x3".into()],
        }];

        let build = snip29_params_to_value(&paymaster_build_deploy_and_invoke(
            &account,
            &pk_low,
            &pk_high,
            calls.clone(),
        ))
        .expect("serialize build");
        let build_s = build.to_string();
        assert!(
            build_s.contains("user_address"),
            "build must emit user_address: {build_s}"
        );
        assert!(
            build_s.contains("\"to\""),
            "build calls must emit to: {build_s}"
        );
        assert!(
            build_s.contains("fee_mode"),
            "build must emit fee_mode: {build_s}"
        );
        assert!(
            !build_s.contains("userAddress"),
            "must not emit camelCase userAddress: {build_s}"
        );
        assert!(
            !build_s.contains("contractAddress"),
            "must not emit camelCase contractAddress: {build_s}"
        );
        assert!(
            !build_s.contains("feeMode"),
            "must not emit camelCase feeMode: {build_s}"
        );
        assert!(
            !build_s.contains("entrypoint"),
            "must not emit entrypoint (use selector): {build_s}"
        );
        assert_eq!(
            build["parameters"]["fee_mode"]["mode"], "sponsored",
            "sponsored fee mode must remain"
        );
        assert_eq!(
            build["transaction"]["deployment"]["calldata"],
            json!([pk_low.to_fixed_hex_string(), pk_high.to_fixed_hex_string()]),
            "constructor must stay [pk_low, pk_high]"
        );

        let invoke_build =
            snip29_params_to_value(&paymaster_build_invoke(&account, calls)).expect("invoke");
        let invoke_s = invoke_build.to_string();
        assert!(invoke_s.contains("user_address"));
        assert!(invoke_s.contains("fee_mode"));
        assert!(!invoke_s.contains("userAddress"));
        assert!(!invoke_s.contains("feeMode"));

        let typed = json!({"types": {}, "primaryType": "OutsideExecution"});
        let execute = snip29_params_to_value(&paymaster_execute_deploy_and_invoke(
            &account,
            &pk_low,
            &pk_high,
            typed.clone(),
            vec!["0xaa".into(), "0xbb".into()],
        ))
        .expect("serialize execute");
        let execute_s = execute.to_string();
        assert!(
            execute_s.contains("user_address"),
            "execute must emit user_address: {execute_s}"
        );
        assert!(
            execute_s.contains("typed_data"),
            "execute must emit typed_data: {execute_s}"
        );
        assert!(
            execute_s.contains("fee_mode"),
            "execute must emit fee_mode: {execute_s}"
        );
        assert!(
            !execute_s.contains("userAddress"),
            "must not emit camelCase userAddress: {execute_s}"
        );
        assert!(
            !execute_s.contains("typedData"),
            "must not emit camelCase typedData: {execute_s}"
        );
        assert!(
            !execute_s.contains("feeMode"),
            "must not emit camelCase feeMode: {execute_s}"
        );

        let execute_invoke = snip29_params_to_value(&paymaster_execute_invoke(
            &account,
            typed,
            vec!["0xaa".into()],
        ))
        .expect("execute invoke");
        let ei = execute_invoke.to_string();
        assert!(
            ei.contains("user_address") && ei.contains("typed_data") && ei.contains("fee_mode")
        );
        assert!(
            !ei.contains("userAddress") && !ei.contains("typedData") && !ei.contains("feeMode")
        );
    }

    #[test]
    fn snip29_call_maps_entrypoint_to_selector_hex() {
        let call = PreparedCall {
            contract_address: "0x0787150e306e6eae6e3f79dea881770e8bbff2c1b8eb490f969669ee945b3135"
                .into(),
            entrypoint: "transfer".into(),
            calldata: vec!["0x1".into(), "0x2".into(), "0x0".into()],
        };
        let wire = snip29_call(&call).expect("snip29_call");
        let v = serde_json::to_value(&wire).unwrap();
        assert!(v.get("selector").is_some());
        assert!(v.get("entrypoint").is_none());
        assert!(v.get("contractAddress").is_none());
        assert!(v.get("to").is_some());
        let selector = selector_from_name("transfer").unwrap();
        assert_eq!(v["selector"], selector.to_fixed_hex_string());
        assert_eq!(
            v["to"],
            felt_from_hex(&call.contract_address)
                .unwrap()
                .to_fixed_hex_string()
        );
    }
}
