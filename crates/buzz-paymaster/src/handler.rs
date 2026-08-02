//! Turning a published request event into a submitted transaction and a result.
//!
//! This is the testable core of the service loop. The WebSocket plumbing around it
//! is glue; everything that decides whether money moves lives here, behind two
//! traits ([`Chain`] and [`Submitter`]) so it runs against fakes with no node, no
//! key and no funds.

use buzz_core::kind::KIND_SPONSOR_RESULT;
use buzz_core::sponsorship::{SponsorRequest, SponsorResult};
use nostr::{Event, EventBuilder, EventId, Keys, Kind, PublicKey, Tag, TagKind};
use starknet_core::types::{Call, Felt};

use crate::{service_request, Chain, SponsorError};

/// Seconds of validity a request must have left before it is worth submitting.
///
/// The account checks `execute_before` against the *block* timestamp, which is
/// later than ours by however long inclusion takes. Submitting a payload that
/// expires in the meantime costs the sponsor a fee for a guaranteed revert, so a
/// request whose window is about to close is refused instead — the client can
/// resend with a wider one, which costs nobody anything.
pub const SUBMISSION_MARGIN_SECS: u64 = 30;

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
pub fn d_tag(event: &Event) -> Option<String> {
    event
        .tags
        .iter()
        .find(|t| t.kind() == TagKind::d())
        .and_then(|t| t.content().map(str::to_string))
}

/// The `d` tag a result carries, and the key the sponsor deduplicates on.
///
/// # Why the author is in the key
///
/// A result is authored by the *sponsor*, so its replaceable identity is
/// `(sponsor_pubkey, 30901, d)`. A nonce alone would not do: nonces are chosen
/// per-user and two members picking `0x1` would occupy one slot, so one member's
/// result would silently overwrite another's — and the sponsor's own record of what
/// it had already paid for would lose an entry, which is how it would come to pay
/// twice.
///
/// Deterministic in both directions, so a client can compute the address of its own
/// result before the sponsor has published it, and the sponsor can rebuild its
/// entire dedupe set from the relay after a restart.
pub fn result_d_tag(author_pubkey: &str, nonce: &str) -> String {
    format!("{author_pubkey}:{nonce}")
}

/// Builds the signed kind:30901 event announcing what was done with a request.
///
/// Tags, and what each is for:
/// - `d` — the addressable key, so a resend replaces rather than accumulates.
/// - `p` — the requester, so a client can subscribe by `#p` on its own pubkey
///   without having to know the sponsor's.
/// - `e` — the specific request event, so a client can tell which attempt this
///   answers when it has resent under one nonce.
pub fn build_result_event(
    keys: &Keys,
    author: &PublicKey,
    request_id: EventId,
    result: &SponsorResult,
) -> Result<Event, SponsorError> {
    EventBuilder::new(Kind::Custom(KIND_SPONSOR_RESULT as u16), result.to_json())
        .tag(Tag::identifier(result_d_tag(
            &author.to_hex(),
            result.nonce(),
        )))
        .tag(Tag::public_key(*author))
        .tag(Tag::event(request_id))
        .sign_with_keys(keys)
        .map_err(|e| SponsorError::Config(format!("cannot sign result event: {e}")))
}

/// Services one request event, always producing a result worth publishing.
///
/// Returns a [`SponsorResult`] rather than an error: every outcome — including
/// refusal — is something the requesting client needs to see. A request that
/// silently vanishes is indistinguishable from a paymaster that is down.
///
/// The author pubkey comes from the event, which the relay has already verified.
/// Nothing in the payload can redirect the account.
///
/// `now` is Unix seconds, passed in rather than read here so the validity-window
/// check is deterministic under test.
pub async fn handle_request_event(
    event: &Event,
    chain: &impl Chain,
    submitter: &impl Submitter,
    class_hash: Felt,
    udc: Felt,
    now: u64,
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

    // The validity window is checked before the account is derived, so an expired
    // request costs neither a chain round trip nor a fee. This is what makes it safe
    // to replay stored requests after a restart: anything whose window has closed is
    // refused for free, so the only requests a restart can re-service are ones still
    // live enough that servicing them is the correct outcome anyway.
    if request.execute_before <= now.saturating_add(SUBMISSION_MARGIN_SECS) {
        return SponsorResult::Declined {
            nonce: request.nonce,
            reason: format!(
                "expired or too close to expiry: execute_before {} against now {now} \
                 (needs {SUBMISSION_MARGIN_SECS}s of margin for inclusion)",
                request.execute_before
            ),
        };
    }
    if request.execute_after >= now {
        // The block timestamp at inclusion is at least `now`, so a window that has
        // not opened yet would revert. The sponsor has no scheduler; refusing lets
        // the client resend when it is due.
        return SponsorResult::Declined {
            nonce: request.nonce,
            reason: format!(
                "not yet valid: execute_after {} against now {now}",
                request.execute_after
            ),
        };
    }

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

    /// Inside every fixture's `[1000, 2000]` window, with margin to spare.
    const NOW: u64 = 1_500;

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
            NOW,
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
            NOW,
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
            NOW,
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
            NOW,
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
            NOW,
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
            NOW,
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

    /// A submitter that fails the test if it is ever reached.
    ///
    /// Used to prove a refusal happened *before* anything could be paid for, which a
    /// `Declined` result alone would not show.
    struct NeverSubmit;
    impl Submitter for NeverSubmit {
        async fn submit(&self, _c: Vec<Call>) -> Result<Felt, SponsorError> {
            panic!("nothing should reach submission");
        }
    }

    #[tokio::test]
    async fn an_expired_request_is_refused_without_paying() {
        // The account checks execute_before against the block timestamp, so this
        // would be a guaranteed revert the sponsor still pays for.
        let r = handle_request_event(
            &event(payload("0x2a"), Some("0x2a")),
            &Chain0(true),
            &NeverSubmit,
            class(),
            crate::UDC_MAINNET,
            5_000,
        )
        .await;
        match r {
            SponsorResult::Declined { reason, .. } => {
                assert!(reason.contains("expired"), "{reason}")
            }
            other => panic!("expected Declined, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_window_closing_inside_the_margin_is_refused() {
        // Still technically valid at `now`, but not for long enough to survive
        // inclusion — so submitting is a coin flip the sponsor pays for either way.
        let now = 2_000 - SUBMISSION_MARGIN_SECS;
        let r = handle_request_event(
            &event(payload("0x2a"), Some("0x2a")),
            &Chain0(true),
            &NeverSubmit,
            class(),
            crate::UDC_MAINNET,
            now,
        )
        .await;
        assert!(matches!(r, SponsorResult::Declined { .. }));
    }

    #[tokio::test]
    async fn a_window_that_has_not_opened_yet_is_refused() {
        let r = handle_request_event(
            &event(payload("0x2a"), Some("0x2a")),
            &Chain0(true),
            &NeverSubmit,
            class(),
            crate::UDC_MAINNET,
            500,
        )
        .await;
        match r {
            SponsorResult::Declined { reason, .. } => {
                assert!(reason.contains("not yet valid"), "{reason}")
            }
            other => panic!("expected Declined, got {other:?}"),
        }
    }

    #[test]
    fn a_result_key_separates_two_members_who_pick_the_same_nonce() {
        // The bug this prevents: results are authored by the sponsor, so a
        // nonce-only d tag would put both members in one replaceable slot and lose
        // the sponsor's record of one of the payments it had already made.
        assert_ne!(result_d_tag("aa", "0x1"), result_d_tag("bb", "0x1"));
        // And it is stable across resends of one nonce, which is what makes
        // replacement idempotent rather than merely unique.
        assert_eq!(result_d_tag("aa", "0x1"), result_d_tag("aa", "0x1"));
    }

    #[test]
    fn a_result_event_is_addressed_so_the_requester_can_find_it() {
        let request = event(payload("0x2a"), Some("0x2a"));
        let sponsor = Keys::generate();
        let result = SponsorResult::Submitted {
            nonce: "0x2a".into(),
            transaction_hash: "0xabc".into(),
            deployed: true,
        };
        let published = build_result_event(&sponsor, &request.pubkey, request.id, &result).unwrap();

        assert_eq!(published.kind, Kind::Custom(30901));
        assert_eq!(
            published.pubkey,
            sponsor.public_key(),
            "the sponsor authors its own results"
        );
        assert_eq!(
            d_tag(&published).unwrap(),
            result_d_tag(&request.pubkey.to_hex(), "0x2a")
        );
        // The requester must be able to filter on #p without knowing the sponsor.
        assert!(published
            .tags
            .iter()
            .any(|t| t.content() == Some(request.pubkey.to_hex().as_str())));
        // And to tell which attempt this answers.
        assert!(published
            .tags
            .iter()
            .any(|t| t.content() == Some(request.id.to_hex().as_str())));
        assert_eq!(
            serde_json::from_str::<SponsorResult>(&published.content).unwrap(),
            result
        );
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
                NOW,
            )
            .await;
            assert_eq!(r.nonce(), "0x7");
        }
    }
}
