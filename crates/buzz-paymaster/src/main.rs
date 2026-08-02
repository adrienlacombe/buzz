//! The sponsor daemon.
//!
//! Reads its configuration from the environment, connects out to the relay, and
//! serves kind:30900 sponsorship requests until stopped. It listens on no port.
//!
//! # Required environment
//!
//! ```text
//! BUZZ_PAYMASTER_RELAY_URL            wss://relay.bitcoinmarkets.app
//! BUZZ_PAYMASTER_RPC_URL              https://mainnet.nodes.starknet.org/rpc/v0_10
//! BUZZ_PAYMASTER_ACCOUNT_CLASS_HASH   the NostrAccount class to deploy
//! BUZZ_PAYMASTER_NOSTR_KEY            relay identity (spends nothing)
//! BUZZ_PAYMASTER_STARKNET_ADDRESS     the funded sponsor account
//! BUZZ_PAYMASTER_STARKNET_KEY         its signing key — the credential that spends
//! ```
//!
//! Optional: `BUZZ_PAYMASTER_UDC`, `BUZZ_PAYMASTER_AUTH_TAG`,
//! `BUZZ_PAYMASTER_MAX_FEE_FRI`, `RUST_LOG`.
//!
//! # Run one instance
//!
//! Two would take the same account nonce and both service a request that arrives
//! before either has published its result. There is no lock; see
//! [`buzz_paymaster::service`].

use buzz_paymaster::config::Config;
use buzz_paymaster::rpc::JsonRpcChain;
use buzz_paymaster::service::{Sponsor, SponsorState, SystemClock};
use buzz_paymaster::submitter::LocalSubmitter;
use tracing::info;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            // The message names the variable or endpoint at fault and never a key
            // value; see `Config::from_env` and `LocalSubmitter::from_env`.
            tracing::error!(error = %e, "sponsor could not start");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), buzz_paymaster::SponsorError> {
    let config = Config::from_env()?;
    info!(?config, "configuration loaded");

    // The submitter reads the chain id from the node and hands it back, so the
    // request-side chain check and the signing-side chain id are the same number by
    // construction rather than by two configuration values agreeing.
    let (submitter, chain_id) = LocalSubmitter::from_env(&config.rpc_url).await?;
    let chain = JsonRpcChain::new(&config.rpc_url)?;

    let sponsor = Sponsor::from_config(&config, chain_id, chain, submitter, SystemClock);
    let mut state = SponsorState::new();

    // Runs until a configuration error makes reconnecting pointless; every transport
    // failure is retried with backoff instead.
    buzz_paymaster::ws::serve(&sponsor, &config, &mut state).await
}
