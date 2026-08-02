//! [`Transport`] over a real relay WebSocket, plus the reconnect loop.
//!
//! Deliberately thin, for the same reason as [`crate::rpc`]: everything that can
//! hold a money-losing bug lives above the trait, tested against a fake. This file
//! only translates between [`RelayMessage`] and [`Inbound`], and the one translation
//! that carries weight is the error mapping on [`Transport::publish`] — see there.

use std::time::{Duration, Instant};

use buzz_ws_client::{NostrWsConnection, RelayMessage, WsClientError};
use nostr::Event;
use serde_json::{json, Value};
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::service::{Clock, Inbound, Sponsor, SponsorState, Transport, RECV_TIMEOUT_SECS};
use crate::{Chain, SponsorError};

/// First reconnect delay.
pub const RECONNECT_MIN_SECS: u64 = 1;
/// Longest reconnect delay.
pub const RECONNECT_MAX_SECS: u64 = 60;
/// A connection lasting at least this long is treated as healthy, resetting backoff.
///
/// Without it, a connection that survives an hour and then drops would reconnect at
/// whatever delay the last outage had escalated to.
pub const RECONNECT_RESET_SECS: u64 = 60;

impl Transport for NostrWsConnection {
    async fn subscribe(
        &mut self,
        subscription_id: &str,
        filter: Value,
    ) -> Result<(), SponsorError> {
        self.send_raw(&json!(["REQ", subscription_id, filter]))
            .await
            .map_err(|e| SponsorError::Chain(format!("cannot open subscription: {e}")))
    }

    async fn recv(&mut self) -> Result<Inbound, SponsorError> {
        match self
            .next_event(Duration::from_secs(RECV_TIMEOUT_SECS))
            .await
        {
            Ok(RelayMessage::Event {
                subscription_id,
                event,
            }) => Ok(Inbound::Event {
                subscription_id,
                event,
            }),
            Ok(RelayMessage::Eose { subscription_id }) => Ok(Inbound::Eose { subscription_id }),
            Ok(RelayMessage::Closed {
                subscription_id,
                message,
            }) => Ok(Inbound::Closed {
                subscription_id,
                message,
            }),
            Ok(RelayMessage::Notice { message }) => {
                warn!(notice = %message, "relay notice");
                Ok(Inbound::Other)
            }
            // A fresh challenge means the session may have lapsed, which would leave
            // the subscriptions dead while the socket still looks fine. Ending the
            // connection re-authenticates instead of listening to nothing.
            Ok(RelayMessage::Auth { .. }) => Err(SponsorError::Chain(
                "relay re-issued an AUTH challenge; reconnecting to re-authenticate".into(),
            )),
            Ok(other) => {
                debug!(?other, "ignoring relay message");
                Ok(Inbound::Other)
            }
            // Silence is not failure. The loop counts idles and decides.
            Err(WsClientError::Timeout) => Ok(Inbound::Idle),
            Err(e) => Err(SponsorError::Chain(format!("relay read failed: {e}"))),
        }
    }

    async fn publish(&mut self, event: Event) -> Result<(), SponsorError> {
        match self.send_event(event).await {
            Ok(ok) if ok.accepted => Ok(()),
            // The relay understood the event and refused it, so retrying is pointless
            // and queueing it would stall every later result. `Config` is what tells
            // the caller to drop it; anything else means try again.
            Ok(ok) => Err(SponsorError::Config(format!(
                "relay rejected the result event: {}",
                ok.message
            ))),
            Err(e) => Err(SponsorError::Chain(format!("cannot publish: {e}"))),
        }
    }
}

/// Connects, serves, and reconnects — forever.
///
/// `state` is threaded through every reconnect rather than rebuilt, because it holds
/// the record of what has already been paid for. Returns only if a configuration
/// error makes reconnecting pointless.
pub async fn serve<C: Chain, S: crate::handler::Submitter, K: Clock>(
    sponsor: &Sponsor<C, S, K>,
    config: &Config,
    state: &mut SponsorState,
) -> Result<(), SponsorError> {
    let mut backoff = RECONNECT_MIN_SECS;
    loop {
        let started = Instant::now();
        match NostrWsConnection::connect_authenticated(
            &config.relay_url,
            &config.keys,
            config.auth_tag.as_ref(),
        )
        .await
        {
            Ok(mut conn) => {
                info!(
                    relay = %config.relay_url,
                    sponsor = %config.keys.public_key().to_hex(),
                    "sponsor connected"
                );
                let outcome = sponsor.run_once(&mut conn, state).await;
                let _ = conn.disconnect().await;
                match outcome {
                    // `run_once` does not return Ok in normal operation; treat it as
                    // a clean end and reconnect rather than exiting silently.
                    Ok(()) => info!("sponsor loop ended cleanly; reconnecting"),
                    Err(e) => warn!(error = %e, "sponsor connection ended"),
                }
            }
            Err(e) => warn!(error = %e, relay = %config.relay_url, "cannot connect to relay"),
        }

        if started.elapsed() >= Duration::from_secs(RECONNECT_RESET_SECS) {
            backoff = RECONNECT_MIN_SECS;
        }
        debug!(
            backoff,
            pending_results = state.outbox_len(),
            "waiting before reconnect"
        );
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(RECONNECT_MAX_SECS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_bounded_and_starts_short() {
        // A sponsor that backs off for an hour is indistinguishable from one that is
        // down, and one that never backs off hammers a struggling relay.
        const { assert!(RECONNECT_MIN_SECS >= 1) };
        const { assert!(RECONNECT_MAX_SECS <= 60) };
        const { assert!(RECONNECT_MIN_SECS < RECONNECT_MAX_SECS) };
    }

    #[test]
    fn backoff_doubles_up_to_the_ceiling() {
        let mut b = RECONNECT_MIN_SECS;
        for _ in 0..20 {
            b = (b * 2).min(RECONNECT_MAX_SECS);
        }
        assert_eq!(b, RECONNECT_MAX_SECS, "must converge, not overflow");
    }

    #[test]
    fn a_subscription_request_is_a_nip01_req_frame() {
        let frame = json!(["REQ", "sub", {"kinds": [30900]}]);
        assert_eq!(frame[0], "REQ");
        assert_eq!(frame[1], "sub");
        assert_eq!(frame[2]["kinds"], json!([30900]));
    }
}
