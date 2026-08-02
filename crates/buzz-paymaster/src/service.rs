//! The service loop: subscribe to requests, service them, publish results.
//!
//! # Why the transport is a trait
//!
//! The loop's job is almost entirely bookkeeping about money already spent —
//! which requests have been paid for, which answers are still owed. Those are the
//! bugs worth catching, and none of them need a socket to reproduce. So
//! [`Transport`] exists to let the whole loop run against a scripted fake, with
//! [`NostrWsConnection`](buzz_ws_client::NostrWsConnection) as the one thin
//! implementation that talks to a relay.
//!
//! # How paying twice is prevented
//!
//! A stored request is replayed to every new subscription, so "service what arrives"
//! is not enough on its own. Three mechanisms stack, in order of how much they cost
//! when they fire:
//!
//! 1. **The validity window**, checked before any chain query
//!    ([`handle_request_event`]). An expired request is refused for free, which is
//!    what makes replay safe in general: the only requests a restart can re-service
//!    are ones still live enough that servicing them is correct.
//! 2. **A dedupe set**, rebuilt at every connect from the sponsor's own published
//!    results. Costs one subscription.
//! 3. **The on-chain nonce**, single-use. Costs a reverted transaction's fee — the
//!    backstop, not the plan.
//!
//! # The one hazard that remains
//!
//! A crash *between* submitting a transaction and publishing its result loses the
//! only record that the sponsor paid, because the record is the published event. On
//! restart that request could be serviced again — bounded by mechanism 1, so a
//! client that asks for a short window is protected by construction and one that
//! asks for hours is not. Closing it properly needs durable state written before
//! submission, which is a store this service does not yet have.
//!
//! An unpublished result is retried across reconnects within a process
//! ([`SponsorState::outbox`]), so only an actual crash loses it.
//!
//! # Running two instances is not safe
//!
//! Both would rebuild the same dedupe set and both would service a request that
//! arrives before either has published. There is no lock here; run one.

use std::collections::HashSet;

use buzz_core::kind::{KIND_SPONSOR_REQUEST, KIND_SPONSOR_RESULT};
use buzz_core::sponsorship::SponsorResult;
use nostr::{Event, Keys};
use serde_json::{json, Value};
use starknet_core::types::Felt;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::handler::{build_result_event, d_tag, handle_request_event, result_d_tag, Submitter};
use crate::{Chain, SponsorError};

/// Subscription id for the request stream.
pub const REQUESTS_SUB: &str = "sponsor-requests";
/// Subscription id for the sponsor's own results.
pub const RESULTS_SUB: &str = "sponsor-results";

/// How many stored events either subscription backfills.
///
/// Bounds both the dedupe set rebuilt at connect and the replay of stored requests.
/// Requests older than this window are dropped rather than serviced — acceptable
/// because their validity windows will long since have closed.
pub const BACKFILL_LIMIT: u64 = 500;

/// Seconds of silence before [`Transport::recv`] reports [`Inbound::Idle`].
pub const RECV_TIMEOUT_SECS: u64 = 300;

/// Consecutive idles tolerated before the connection is assumed dead.
///
/// A TCP connection can be gone without either side noticing, and a sponsor that
/// has silently stopped listening is worse than one that is visibly down: requests
/// accumulate and nobody is told. Reconnecting after a quiet stretch is cheap; the
/// wrong-way error here is patience.
pub const MAX_CONSECUTIVE_IDLE: u32 = 3;

/// Wall-clock source, injected so the validity-window check is testable.
pub trait Clock {
    /// Unix seconds.
    fn now_secs(&self) -> u64;
}

/// The real clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_secs(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            // A clock before the epoch yields 0, which makes every request read as
            // "not yet valid" and nothing is paid for. Failing closed on a broken
            // clock is the right direction for a service that spends money.
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// One message from the relay, reduced to what the loop reacts to.
#[derive(Debug)]
pub enum Inbound {
    /// An event on a subscription.
    Event {
        /// Which subscription delivered it.
        subscription_id: String,
        /// The event.
        event: Box<Event>,
    },
    /// A subscription finished replaying stored events.
    Eose {
        /// Which subscription.
        subscription_id: String,
    },
    /// The relay closed a subscription.
    Closed {
        /// Which subscription.
        subscription_id: String,
        /// The relay's reason.
        message: String,
    },
    /// Nothing arrived within the receive timeout.
    Idle,
    /// Something arrived that the loop does not act on.
    Other,
}

/// The relay operations the loop needs.
pub trait Transport {
    /// Opens a subscription.
    fn subscribe(
        &mut self,
        subscription_id: &str,
        filter: Value,
    ) -> impl std::future::Future<Output = Result<(), SponsorError>> + Send;

    /// Waits for the next message.
    ///
    /// Silence must surface as [`Inbound::Idle`] rather than an error, so the loop
    /// can distinguish a quiet relay from a broken one.
    fn recv(&mut self) -> impl std::future::Future<Output = Result<Inbound, SponsorError>> + Send;

    /// Publishes an event.
    ///
    /// The error kind is load-bearing: [`SponsorError::Chain`] means "try again on a
    /// new connection", while [`SponsorError::Config`] means the relay refused the
    /// event itself and retrying would loop forever. Getting this wrong stalls every
    /// later result behind one that can never be delivered.
    fn publish(
        &mut self,
        event: Event,
    ) -> impl std::future::Future<Output = Result<(), SponsorError>> + Send;
}

/// What the sponsor must remember between connections.
///
/// Owned by the caller and **not** reset on reconnect: that is the point. Clearing
/// it would discard the record of what has already been paid for, and the next
/// replay would pay again.
#[derive(Debug, Default)]
pub struct SponsorState {
    /// Result keys ([`result_d_tag`]) known to have been submitted and paid for.
    submitted: HashSet<String>,
    /// Results built but not yet accepted by the relay.
    outbox: Vec<Event>,
}

impl SponsorState {
    /// A fresh state, having paid for nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many requests are known to have been paid for.
    pub fn submitted_count(&self) -> usize {
        self.submitted.len()
    }

    /// How many results are still owed to the relay.
    pub fn outbox_len(&self) -> usize {
        self.outbox.len()
    }

    /// Whether this result key has already been paid for.
    pub fn is_submitted(&self, key: &str) -> bool {
        self.submitted.contains(key)
    }
}

/// The sponsor: everything needed to service requests except a connection.
pub struct Sponsor<C, S, K> {
    /// Chain reads.
    pub chain: C,
    /// Spending authority — the only component holding a funded key.
    pub submitter: S,
    /// Wall clock.
    pub clock: K,
    /// Class hash accounts are deployed from.
    pub class_hash: Felt,
    /// Universal Deployer address.
    pub udc: Felt,
    /// The sponsor's Nostr identity, used to sign results.
    pub keys: Keys,
}

impl<C: Chain, S: Submitter, K: Clock> Sponsor<C, S, K> {
    /// Builds a sponsor from config plus the two components that touch the outside
    /// world.
    pub fn from_config(config: &Config, chain: C, submitter: S, clock: K) -> Self {
        Self {
            chain,
            submitter,
            clock,
            class_hash: config.class_hash,
            udc: config.udc,
            keys: config.keys.clone(),
        }
    }

    /// The filter for the sponsor's own results, used to rebuild the dedupe set.
    fn results_filter(&self) -> Value {
        json!({
            "kinds": [KIND_SPONSOR_RESULT],
            "authors": [self.keys.public_key().to_hex()],
            "limit": BACKFILL_LIMIT,
        })
    }

    /// The filter for incoming requests.
    ///
    /// Deliberately unrestricted by author: membership is the gate on who may ask,
    /// and the relay has already applied it. Deliberately unrestricted by `since`
    /// too — a request published while the sponsor was down must still be serviced,
    /// and the validity-window check is what keeps replaying old ones free.
    fn requests_filter(&self) -> Value {
        json!({
            "kinds": [KIND_SPONSOR_REQUEST],
            "limit": BACKFILL_LIMIT,
        })
    }

    /// Serves one connection until it fails or goes quiet.
    ///
    /// Returns `Err` for anything the caller should reconnect after. It does not
    /// return `Ok` in normal operation.
    pub async fn run_once(
        &self,
        transport: &mut impl Transport,
        state: &mut SponsorState,
    ) -> Result<(), SponsorError> {
        // Answers owed from a previous connection go out before anything new is
        // serviced. A sponsor that starts spending again while still owing a receipt
        // for the last thing it spent on is how a record gets lost.
        self.flush_outbox(transport, state).await?;

        transport
            .subscribe(RESULTS_SUB, self.results_filter())
            .await?;
        self.drain_until_eose(transport, RESULTS_SUB, state).await?;
        info!(
            submitted = state.submitted_count(),
            "rebuilt the sponsorship dedupe set from published results"
        );

        transport
            .subscribe(REQUESTS_SUB, self.requests_filter())
            .await?;

        let mut idle = 0u32;
        loop {
            match transport.recv().await? {
                Inbound::Event {
                    subscription_id,
                    event,
                } => {
                    idle = 0;
                    match subscription_id.as_str() {
                        REQUESTS_SUB => self.service(transport, state, &event).await?,
                        // The results subscription stays open after EOSE, so the
                        // sponsor's own publishes echo back here. Absorbing them
                        // keeps the set correct without a second source of truth.
                        RESULTS_SUB => absorb_result(state, &event),
                        other => debug!(subscription_id = other, "ignoring unknown subscription"),
                    }
                }
                Inbound::Eose { .. } => idle = 0,
                Inbound::Closed {
                    subscription_id,
                    message,
                } => {
                    return Err(SponsorError::Chain(format!(
                        "relay closed subscription {subscription_id}: {message}"
                    )));
                }
                Inbound::Idle => {
                    idle += 1;
                    if idle >= MAX_CONSECUTIVE_IDLE {
                        return Err(SponsorError::Chain(format!(
                            "relay silent for {}s; reconnecting rather than \
                             listening to a connection that may be gone",
                            RECV_TIMEOUT_SECS * u64::from(idle)
                        )));
                    }
                    debug!(idle, "relay quiet");
                }
                Inbound::Other => {}
            }
        }
    }

    /// Reads stored events on `subscription_id` until the relay signals EOSE.
    async fn drain_until_eose(
        &self,
        transport: &mut impl Transport,
        subscription_id: &str,
        state: &mut SponsorState,
    ) -> Result<(), SponsorError> {
        let mut idle = 0u32;
        loop {
            match transport.recv().await? {
                Inbound::Eose {
                    subscription_id: id,
                } if id == subscription_id => return Ok(()),
                Inbound::Event { event, .. } => {
                    idle = 0;
                    absorb_result(state, &event);
                }
                Inbound::Closed { message, .. } => {
                    // Failing here rather than proceeding is deliberate: servicing
                    // requests with a half-built dedupe set is how the sponsor pays
                    // twice.
                    return Err(SponsorError::Chain(format!(
                        "relay closed {subscription_id} during backfill: {message}"
                    )));
                }
                Inbound::Idle => {
                    idle += 1;
                    if idle >= MAX_CONSECUTIVE_IDLE {
                        return Err(SponsorError::Chain(format!(
                            "no EOSE for {subscription_id}; refusing to service \
                             requests without a complete dedupe set"
                        )));
                    }
                }
                _ => {}
            }
        }
    }

    /// Services one request event and publishes the outcome.
    async fn service(
        &self,
        transport: &mut impl Transport,
        state: &mut SponsorState,
        event: &Event,
    ) -> Result<(), SponsorError> {
        let author = event.pubkey;
        // The request's own d tag is its nonce, and a well-formed request is only
        // ever submitted when the two agree, so this key matches the one the stored
        // result carries. A malformed request that never reached submission is not in
        // the set at all, and re-refusing it is free.
        let key = result_d_tag(&author.to_hex(), &d_tag(event).unwrap_or_default());
        if state.is_submitted(&key) {
            debug!(key, "already sponsored; the result is already published");
            return Ok(());
        }

        let result = handle_request_event(
            event,
            &self.chain,
            &self.submitter,
            self.class_hash,
            self.udc,
            self.clock.now_secs(),
        )
        .await;

        // Recorded *before* publishing. If publishing fails, the sponsor has still
        // spent the money, and the process must not be able to spend it again.
        if let SponsorResult::Submitted {
            transaction_hash, ..
        } = &result
        {
            let paid = result_d_tag(&author.to_hex(), result.nonce());
            debug_assert_eq!(paid, key, "the dedupe key must match the published d tag");
            state.submitted.insert(paid);
            info!(
                tx = %transaction_hash,
                requester = %author.to_hex(),
                "sponsored a transaction"
            );
        } else if let SponsorResult::Declined { reason, .. } = &result {
            info!(requester = %author.to_hex(), reason = %reason, "declined a sponsorship request");
        }

        let signed = build_result_event(&self.keys, &author, event.id, &result)?;
        self.publish_result(transport, state, signed).await
    }

    /// Publishes a result, queueing it for retry if the relay could not take it.
    async fn publish_result(
        &self,
        transport: &mut impl Transport,
        state: &mut SponsorState,
        signed: Event,
    ) -> Result<(), SponsorError> {
        match transport.publish(signed.clone()).await {
            Ok(()) => Ok(()),
            // The relay refused the event itself. Retrying cannot help, and holding
            // it in the outbox would block every later result behind it.
            Err(SponsorError::Config(reason)) => {
                error!(
                    reason = %reason,
                    event_id = %signed.id.to_hex(),
                    "relay rejected a sponsorship result; dropping it. The transaction \
                     may have been submitted and the requester will not learn of it here"
                );
                Ok(())
            }
            Err(e) => {
                warn!(error = %e, "could not publish a sponsorship result; queued for retry");
                state.outbox.push(signed);
                Err(e)
            }
        }
    }

    /// Retries results left unpublished by an earlier connection.
    async fn flush_outbox(
        &self,
        transport: &mut impl Transport,
        state: &mut SponsorState,
    ) -> Result<(), SponsorError> {
        if state.outbox.is_empty() {
            return Ok(());
        }
        info!(pending = state.outbox.len(), "retrying unpublished results");
        for signed in std::mem::take(&mut state.outbox) {
            self.publish_result(transport, state, signed).await?;
        }
        Ok(())
    }
}

/// Records a published result in the dedupe set.
///
/// Only `submitted` results count. A `declined` one means nothing was paid for, so
/// re-servicing is both safe and often right — a request refused because the chain
/// was unreachable should succeed on a later attempt.
fn absorb_result(state: &mut SponsorState, event: &Event) {
    let Some(key) = d_tag(event) else {
        warn!(event_id = %event.id.to_hex(), "a stored result has no d tag; ignoring");
        return;
    };
    match serde_json::from_str::<SponsorResult>(&event.content) {
        Ok(SponsorResult::Submitted { .. }) => {
            state.submitted.insert(key);
        }
        Ok(SponsorResult::Declined { .. }) => {}
        Err(e) => warn!(error = %e, key, "a stored result is unparseable; ignoring"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::SUBMISSION_MARGIN_SECS;
    use nostr::{EventBuilder, Kind, Tag};
    use starknet_core::types::Call;
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const CLASS_HASH: &str = "0x0414f62ea1ed35f8c7bd3b794d94efc95e01bccf04e0f47211fc198f7f56f537";
    const NOW: u64 = 1_500;

    struct Chain0(bool);
    impl Chain for Chain0 {
        async fn is_deployed(&self, _a: Felt) -> Result<bool, SponsorError> {
            Ok(self.0)
        }
    }

    /// Counts submissions, which is the number these tests are really about: a
    /// second submission for one request is money gone.
    #[derive(Default)]
    struct CountingSubmit(AtomicUsize);
    impl Submitter for CountingSubmit {
        async fn submit(&self, _c: Vec<Call>) -> Result<Felt, SponsorError> {
            let n = self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Felt::from(0xf00u32 + n as u32))
        }
    }

    struct FixedClock(u64);
    impl Clock for FixedClock {
        fn now_secs(&self) -> u64 {
            self.0
        }
    }

    /// A scripted relay. `inbox` is delivered in order, then `Idle` forever.
    struct FakeRelay {
        inbox: RefCell<std::collections::VecDeque<Inbound>>,
        published: RefCell<Vec<Event>>,
        subscribed: RefCell<Vec<(String, Value)>>,
        /// Publishes to fail with, popped per call: `None` succeeds.
        publish_errors: RefCell<std::collections::VecDeque<Option<SponsorError>>>,
    }

    impl FakeRelay {
        fn new(inbox: Vec<Inbound>) -> Self {
            Self {
                inbox: RefCell::new(inbox.into()),
                published: RefCell::new(Vec::new()),
                subscribed: RefCell::new(Vec::new()),
                publish_errors: RefCell::new(std::collections::VecDeque::new()),
            }
        }
        fn failing_publishes(self, errs: Vec<Option<SponsorError>>) -> Self {
            *self.publish_errors.borrow_mut() = errs.into();
            self
        }
    }

    impl Transport for FakeRelay {
        async fn subscribe(&mut self, sub: &str, filter: Value) -> Result<(), SponsorError> {
            self.subscribed.borrow_mut().push((sub.to_string(), filter));
            Ok(())
        }
        async fn recv(&mut self) -> Result<Inbound, SponsorError> {
            Ok(self.inbox.borrow_mut().pop_front().unwrap_or(Inbound::Idle))
        }
        async fn publish(&mut self, event: Event) -> Result<(), SponsorError> {
            if let Some(Some(e)) = self.publish_errors.borrow_mut().pop_front() {
                return Err(e);
            }
            self.published.borrow_mut().push(event);
            Ok(())
        }
    }

    fn sponsor(deployed: bool, now: u64) -> Sponsor<Chain0, CountingSubmit, FixedClock> {
        Sponsor {
            chain: Chain0(deployed),
            submitter: CountingSubmit::default(),
            clock: FixedClock(now),
            class_hash: Felt::from_hex_unchecked(CLASS_HASH),
            udc: crate::UDC_MAINNET,
            keys: Keys::generate(),
        }
    }

    fn payload(nonce: &str) -> String {
        json!({
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

    /// A request event from a freshly generated member key.
    fn request(keys: &Keys, nonce: &str) -> Event {
        EventBuilder::new(Kind::Custom(KIND_SPONSOR_REQUEST as u16), payload(nonce))
            .tag(Tag::identifier(nonce))
            .sign_with_keys(keys)
            .expect("sign")
    }

    fn inbound(sub: &str, event: Event) -> Inbound {
        Inbound::Event {
            subscription_id: sub.to_string(),
            event: Box::new(event),
        }
    }

    fn eose(sub: &str) -> Inbound {
        Inbound::Eose {
            subscription_id: sub.to_string(),
        }
    }

    /// Runs one connection to completion, returning the relay and the error that
    /// ended it. `run_once` never returns `Ok`, so an error is the normal exit.
    async fn run(
        s: &Sponsor<Chain0, CountingSubmit, FixedClock>,
        state: &mut SponsorState,
        inbox: Vec<Inbound>,
    ) -> (FakeRelay, SponsorError) {
        let mut relay = FakeRelay::new(inbox);
        let err = s
            .run_once(&mut relay, state)
            .await
            .expect_err("ends on idle");
        (relay, err)
    }

    #[tokio::test]
    async fn a_request_is_serviced_and_its_result_published() {
        let s = sponsor(false, NOW);
        let member = Keys::generate();
        let req = request(&member, "0x2a");
        let mut state = SponsorState::new();

        let (relay, _) = run(
            &s,
            &mut state,
            vec![eose(RESULTS_SUB), inbound(REQUESTS_SUB, req.clone())],
        )
        .await;

        assert_eq!(s.submitter.0.load(Ordering::SeqCst), 1, "submitted once");
        let published = relay.published.borrow();
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].kind, Kind::Custom(KIND_SPONSOR_RESULT as u16));
        assert_eq!(
            d_tag(&published[0]).unwrap(),
            result_d_tag(&member.public_key().to_hex(), "0x2a")
        );
        let result: SponsorResult = serde_json::from_str(&published[0].content).unwrap();
        assert!(matches!(
            result,
            SponsorResult::Submitted { deployed: true, .. }
        ));
    }

    #[tokio::test]
    async fn the_results_subscription_is_drained_before_requests_are_serviced() {
        // Order is the safety property: subscribing to requests first would let one
        // be serviced against an empty dedupe set.
        let s = sponsor(true, NOW);
        let mut state = SponsorState::new();
        let (relay, _) = run(&s, &mut state, vec![eose(RESULTS_SUB)]).await;
        let subs = relay.subscribed.borrow();
        assert_eq!(subs[0].0, RESULTS_SUB);
        assert_eq!(subs[1].0, REQUESTS_SUB);
    }

    #[tokio::test]
    async fn a_replayed_request_is_not_paid_for_twice() {
        // The core money bug: the same stored request is delivered on every new
        // subscription.
        let s = sponsor(true, NOW);
        let member = Keys::generate();
        let req = request(&member, "0x2a");
        let mut state = SponsorState::new();

        run(
            &s,
            &mut state,
            vec![
                eose(RESULTS_SUB),
                inbound(REQUESTS_SUB, req.clone()),
                inbound(REQUESTS_SUB, req.clone()),
            ],
        )
        .await;

        assert_eq!(
            s.submitter.0.load(Ordering::SeqCst),
            1,
            "the second delivery must not spend anything"
        );
    }

    #[tokio::test]
    async fn a_reconnect_rebuilds_the_dedupe_set_from_published_results() {
        // What protects a restarted process: the record lives on the relay.
        let s = sponsor(true, NOW);
        let member = Keys::generate();
        let req = request(&member, "0x2a");

        let mut first = SponsorState::new();
        let (relay, _) = run(
            &s,
            &mut first,
            vec![eose(RESULTS_SUB), inbound(REQUESTS_SUB, req.clone())],
        )
        .await;
        let stored_result = relay.published.borrow()[0].clone();

        // A brand-new state, as after a restart, replaying both the result and the
        // request.
        let mut fresh = SponsorState::new();
        run(
            &s,
            &mut fresh,
            vec![
                inbound(RESULTS_SUB, stored_result),
                eose(RESULTS_SUB),
                inbound(REQUESTS_SUB, req),
            ],
        )
        .await;

        assert_eq!(
            s.submitter.0.load(Ordering::SeqCst),
            1,
            "a restart must not re-pay for a request whose result is on the relay"
        );
        assert_eq!(fresh.submitted_count(), 1);
    }

    #[tokio::test]
    async fn a_declined_result_does_not_block_a_later_attempt() {
        // Nothing was paid for, and the reason may have been transient — a chain
        // that was unreachable should not permanently poison the request.
        let s = sponsor(true, NOW);
        let member = Keys::generate();
        let declined = build_result_event(
            &s.keys,
            &member.public_key(),
            request(&member, "0x2a").id,
            &SponsorResult::Declined {
                nonce: "0x2a".into(),
                reason: "chain query failed".into(),
            },
        )
        .unwrap();

        let mut state = SponsorState::new();
        run(
            &s,
            &mut state,
            vec![
                inbound(RESULTS_SUB, declined),
                eose(RESULTS_SUB),
                inbound(REQUESTS_SUB, request(&member, "0x2a")),
            ],
        )
        .await;

        assert_eq!(s.submitter.0.load(Ordering::SeqCst), 1, "retry is allowed");
        assert_eq!(state.submitted_count(), 1);
    }

    #[tokio::test]
    async fn an_expired_replayed_request_costs_nothing() {
        // Mechanism 1, and the reason replaying stored requests is safe at all.
        let s = sponsor(true, 5_000);
        let member = Keys::generate();
        let mut state = SponsorState::new();
        let (relay, _) = run(
            &s,
            &mut state,
            vec![
                eose(RESULTS_SUB),
                inbound(REQUESTS_SUB, request(&member, "0x2a")),
            ],
        )
        .await;

        assert_eq!(s.submitter.0.load(Ordering::SeqCst), 0);
        let result: SponsorResult =
            serde_json::from_str(&relay.published.borrow()[0].content).unwrap();
        assert!(
            matches!(result, SponsorResult::Declined { .. }),
            "the requester must still be told"
        );
    }

    #[tokio::test]
    async fn two_members_using_the_same_nonce_get_separate_results() {
        // Results are authored by the sponsor, so a nonce-only key would collide and
        // one member's record would overwrite the other's.
        let s = sponsor(true, NOW);
        let (a, b) = (Keys::generate(), Keys::generate());
        let mut state = SponsorState::new();
        let (relay, _) = run(
            &s,
            &mut state,
            vec![
                eose(RESULTS_SUB),
                inbound(REQUESTS_SUB, request(&a, "0x1")),
                inbound(REQUESTS_SUB, request(&b, "0x1")),
            ],
        )
        .await;

        assert_eq!(
            s.submitter.0.load(Ordering::SeqCst),
            2,
            "two distinct requests"
        );
        let published = relay.published.borrow();
        assert_ne!(
            d_tag(&published[0]).unwrap(),
            d_tag(&published[1]).unwrap(),
            "one slot for two members would lose a payment record"
        );
        assert_eq!(state.submitted_count(), 2);
    }

    #[tokio::test]
    async fn an_unpublished_result_is_retried_on_the_next_connection() {
        let s = sponsor(true, NOW);
        let member = Keys::generate();
        let mut state = SponsorState::new();

        // Publishing fails, so the result goes to the outbox and the connection ends.
        let mut relay = FakeRelay::new(vec![
            eose(RESULTS_SUB),
            inbound(REQUESTS_SUB, request(&member, "0x2a")),
        ])
        .failing_publishes(vec![Some(SponsorError::Chain("socket gone".into()))]);
        assert!(s.run_once(&mut relay, &mut state).await.is_err());
        assert_eq!(state.outbox_len(), 1);
        assert!(
            state.is_submitted(&result_d_tag(&member.public_key().to_hex(), "0x2a")),
            "the payment must be recorded even though the receipt was not delivered"
        );

        // The next connection delivers it before servicing anything new.
        let (relay2, _) = run(&s, &mut state, vec![eose(RESULTS_SUB)]).await;
        assert_eq!(state.outbox_len(), 0);
        assert_eq!(relay2.published.borrow().len(), 1);
        assert_eq!(s.submitter.0.load(Ordering::SeqCst), 1, "no second payment");
    }

    #[tokio::test]
    async fn a_result_the_relay_rejects_outright_is_dropped_not_retried_forever() {
        // A permanently unacceptable event in the outbox would stall every later
        // result behind it.
        let s = sponsor(true, NOW);
        let member = Keys::generate();
        let mut state = SponsorState::new();
        let mut relay = FakeRelay::new(vec![
            eose(RESULTS_SUB),
            inbound(REQUESTS_SUB, request(&member, "0x2a")),
        ])
        .failing_publishes(vec![Some(SponsorError::Config("blocked: no scope".into()))]);

        assert!(
            s.run_once(&mut relay, &mut state).await.is_err(),
            "ends on idle"
        );
        assert_eq!(state.outbox_len(), 0, "not queued");
        assert_eq!(state.submitted_count(), 1, "but still recorded as paid");
    }

    #[tokio::test]
    async fn a_closed_subscription_ends_the_connection() {
        let s = sponsor(true, NOW);
        let mut state = SponsorState::new();
        let mut relay = FakeRelay::new(vec![
            eose(RESULTS_SUB),
            Inbound::Closed {
                subscription_id: REQUESTS_SUB.into(),
                message: "rate-limited".into(),
            },
        ]);
        let err = s.run_once(&mut relay, &mut state).await.unwrap_err();
        assert!(format!("{err}").contains("rate-limited"), "{err}");
    }

    #[tokio::test]
    async fn a_backfill_that_never_reaches_eose_refuses_to_service_requests() {
        // Servicing with a half-built dedupe set is how the sponsor pays twice, so
        // this must fail rather than proceed.
        let s = sponsor(true, NOW);
        let mut state = SponsorState::new();
        let mut relay = FakeRelay::new(vec![]); // only Idle
        let err = s.run_once(&mut relay, &mut state).await.unwrap_err();
        assert!(format!("{err}").contains("dedupe set"), "{err}");
        assert!(
            relay
                .subscribed
                .borrow()
                .iter()
                .all(|(s, _)| s == RESULTS_SUB),
            "the request subscription must never have been opened"
        );
    }

    #[tokio::test]
    async fn a_silent_relay_ends_the_connection_so_the_caller_reconnects() {
        // A sponsor that has silently stopped listening is worse than one visibly
        // down: requests pile up and nobody is told.
        let s = sponsor(true, NOW);
        let mut state = SponsorState::new();
        let (_, err) = run(&s, &mut state, vec![eose(RESULTS_SUB)]).await;
        assert!(format!("{err}").contains("silent"), "{err}");
    }

    #[test]
    fn the_filters_ask_for_the_right_kinds() {
        let s = sponsor(true, NOW);
        assert_eq!(s.requests_filter()["kinds"], json!([KIND_SPONSOR_REQUEST]));
        assert_eq!(s.results_filter()["kinds"], json!([KIND_SPONSOR_RESULT]));
        // Only our own results count as a record of what we paid for.
        assert_eq!(
            s.results_filter()["authors"],
            json!([s.keys.public_key().to_hex()])
        );
        // No `since`: a request published while the sponsor was down must still be
        // serviced, and expiry is what keeps replay free.
        assert!(s.requests_filter().get("since").is_none());
    }

    #[test]
    fn the_submission_margin_is_shorter_than_the_test_windows() {
        // Guards the fixtures above: a margin wider than a 1000s window would make
        // every "valid request" test silently assert on expiry instead.
        const { assert!(SUBMISSION_MARGIN_SECS < 1_000) };
    }

    #[test]
    fn a_broken_clock_fails_closed() {
        // now = 0 makes every request read as not-yet-valid, so nothing is paid for.
        assert_eq!(FixedClock(0).now_secs(), 0);
    }
}
