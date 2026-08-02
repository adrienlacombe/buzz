//! Turning a published request event into a submitted transaction and a result.
//!
//! This is the testable core of the service loop. The WebSocket plumbing around it
//! is glue; everything that decides whether money moves lives here, behind two
//! traits ([`Chain`] and [`Submitter`]) so it runs against fakes with no node, no
//! key and no funds.

use buzz_core::sponsorship::{SponsorRequest, SponsorResult};
use nostr::{Event, TagKind};
use starknet_core::types::{Call, Felt};

use crate::{service_request, Chain, SponsorError};

/// Sends a multicall from the sponsor's funded account.
///
/// A trait because the implementation needs a hot key and real fee estimation.
/// Keeping it behind a boundary means the decision logic above is testable, and it
/// is the single place spending authority enters the process.
pub trait Submitter {
    /// Submits `calls` as one transaction, returning its hash.
    fn submit(
        &self,
        calls: Vec<Call>,
    ) -> impl std::future::Future<Output = Result<Felt, SponsorError>> + Send;
}

/// Reads the `d` tag, which carries the SNIP-9 nonce.
fn d_tag(event: &Event) -> Option<String> {
    event
        .tags
        .iter()
        .find(|t| t.kind() == TagKind::d())
        .and_then(|t| t.content().map(str::to_string))
}

/// Services one request event, always producing a result worth publishing.
///
/// Returns a [`SponsorResult`] rather than an error: every outcome — including
/// refusal — is something the requesting client needs to see. A request that
/// silently vanishes is indistinguishable from a paymaster that is down.
///
/// The author pubkey comes from the event, which the relay has already verified.
/// Nothing in the payload can redirect the account.
pub async fn handle_request_event(
    event: &Event,
    chain: &impl Chain,
    submitter: &impl Submitter,
    class_hash: Felt,
    udc: Felt,
) -> SponsorResult {
    let Some(d) = d_tag(event) else {
        // Without a d tag the event is not addressable, so replacement cannot
        // dedupe it and a retry would submit twice.
        return SponsorResult::Declined {
            nonce: String::new(),
            reason: "missing d tag; the request must be addressable by its nonce".into(),
        };
    };

    let request = match SponsorRequest::from_json(&event.content) {
        Ok(r) => r,
        // A stale NIP-SW wallet binding still stored at kind 30900 lands here and is
        // refused rather than misread.
        Err(e) => {
            return SponsorResult::Declined {
                nonce: d,
                reason: e.to_string(),
            };
        }
    };

    let author = event.pubkey.to_hex();
    let calls = match service_request(chain, class_hash, &author, udc, &request, &d).await {
        Ok(calls) => calls,
        Err(e) => {
            return SponsorResult::Declined {
                nonce: request.nonce,
                reason: e.to_string(),
            };
        }
    };
    let deployed = calls.len() > 1;

    match submitter.submit(calls).await {
        Ok(tx) => SponsorResult::Submitted {
            nonce: request.nonce,
            transaction_hash: format!("{tx:#x}"),
            deployed,
        },
        // A submission failure is reported, not retried here. Retrying blind is how
        // a sponsor pays twice for one request; the nonce is single-use on chain, so
        // a client can safely resend under the same d tag once it sees this.
        Err(e) => SponsorResult::Declined {
            nonce: request.nonce,
            reason: format!("submission failed: {e}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    const CLASS_HASH: &str = "0x0414f62ea1ed35f8c7bd3b794d94efc95e01bccf04e0f47211fc198f7f56f537";

    struct Chain0(bool);
    impl Chain for Chain0 {
        async fn is_deployed(&self, _a: Felt) -> Result<bool, SponsorError> {
            Ok(self.0)
        }
    }
    struct OkSubmit;
    impl Submitter for OkSubmit {
        async fn submit(&self, _c: Vec<Call>) -> Result<Felt, SponsorError> {
            Ok(Felt::from_hex_unchecked("0xdeadbeef"))
        }
    }
    struct FailSubmit;
    impl Submitter for FailSubmit {
        async fn submit(&self, _c: Vec<Call>) -> Result<Felt, SponsorError> {
            Err(SponsorError::Chain("insufficient balance".into()))
        }
    }

    fn class() -> Felt {
        Felt::from_hex_unchecked(CLASS_HASH)
    }

    fn payload(nonce: &str) -> String {
        serde_json::json!({
            "chain_id": "SN_MAIN",
            "caller": "0x0",
            "nonce": nonce,
            "execute_after": 1000,
            "execute_before": 2000,
            "calls": [{"to": "0x1234", "selector": "0x5678", "calldata": ["0x1"]}],
            "signature": ["0x1", "0x2", "0x3", "0x4"]
        })
        .to_string()
    }

    /// Signs a request event the way a client would. The keys are generated per
    /// test and never leave it.
    fn event(content: String, d: Option<&str>) -> Event {
        let mut builder = EventBuilder::new(Kind::Custom(30900), content);
        if let Some(d) = d {
            builder = builder.tag(Tag::identifier(d));
        }
        builder.sign_with_keys(&Keys::generate()).expect("sign")
    }

    #[tokio::test]
    async fn a_valid_request_from_a_new_account_reports_deployment() {
        let r = handle_request_event(
            &event(payload("0x2a"), Some("0x2a")),
            &Chain0(false),
            &OkSubmit,
            class(),
            crate::UDC_MAINNET,
        )
        .await;
        match r {
            SponsorResult::Submitted {
                deployed,
                transaction_hash,
                nonce,
            } => {
                assert!(
                    deployed,
                    "an undeployed account must be deployed in the same tx"
                );
                assert_eq!(transaction_hash, "0xdeadbeef");
                assert_eq!(nonce, "0x2a");
            }
            other => panic!("expected Submitted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_existing_account_reports_no_deployment() {
        let r = handle_request_event(
            &event(payload("0x2a"), Some("0x2a")),
            &Chain0(true),
            &OkSubmit,
            class(),
            crate::UDC_MAINNET,
        )
        .await;
        assert!(matches!(
            r,
            SponsorResult::Submitted {
                deployed: false,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn a_missing_d_tag_is_declined() {
        // Not addressable means replacement cannot dedupe, so a retry would pay
        // twice.
        let r = handle_request_event(
            &event(payload("0x2a"), None),
            &Chain0(true),
            &OkSubmit,
            class(),
            crate::UDC_MAINNET,
        )
        .await;
        assert!(matches!(r, SponsorResult::Declined { .. }));
    }

    #[tokio::test]
    async fn a_nonce_that_disagrees_with_the_d_tag_is_declined() {
        let r = handle_request_event(
            &event(payload("0x2a"), Some("0xother")),
            &Chain0(true),
            &OkSubmit,
            class(),
            crate::UDC_MAINNET,
        )
        .await;
        assert!(matches!(r, SponsorResult::Declined { .. }));
    }

    #[tokio::test]
    async fn a_stale_wallet_binding_at_this_kind_is_declined_not_misread() {
        // kind 30900 briefly carried NIP-SW bindings. Valid JSON, wrong shape.
        let r = handle_request_event(
            &event(
                r#"{"chain_id":"SN_MAIN","address":"0x1"}"#.into(),
                Some("SN_MAIN"),
            ),
            &Chain0(true),
            &OkSubmit,
            class(),
            crate::UDC_MAINNET,
        )
        .await;
        assert!(matches!(r, SponsorResult::Declined { .. }));
    }

    #[tokio::test]
    async fn a_submission_failure_is_reported_rather_than_swallowed() {
        // A request that vanishes is indistinguishable from a paymaster that is
        // down, so every outcome must be publishable.
        let r = handle_request_event(
            &event(payload("0x2a"), Some("0x2a")),
            &Chain0(true),
            &FailSubmit,
            class(),
            crate::UDC_MAINNET,
        )
        .await;
        match r {
            SponsorResult::Declined { reason, nonce } => {
                assert!(reason.contains("submission failed"), "got {reason}");
                assert_eq!(nonce, "0x2a");
            }
            other => panic!("expected Declined, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_result_always_carries_the_nonce_it_answers() {
        // The client correlates on this; a result without it is unusable.
        for (content, d) in [
            (payload("0x7"), Some("0x7")),
            ("garbage".to_string(), Some("0x7")),
        ] {
            let r = handle_request_event(
                &event(content, d),
                &Chain0(true),
                &OkSubmit,
                class(),
                crate::UDC_MAINNET,
            )
            .await;
            assert_eq!(r.nonce(), "0x7");
        }
    }
}
