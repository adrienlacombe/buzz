//! Wire format for sponsorship requests and results.
//!
//! A user publishes a [`SponsorRequest`] carrying a SNIP-9 payload they have
//! already signed; the paymaster subscribes as a relay client, pays for it, and
//! publishes a [`SponsorResult`].
//!
//! # Why over Nostr rather than an HTTP endpoint
//!
//! The paymaster holds funds. Going through the relay means it needs **no inbound
//! network surface at all** — it connects out, subscribes, and acts. A funded
//! service with no listening port is a much smaller target than one exposing an
//! authenticated API. It also inherits NIP-42 auth and community membership as the
//! gate on who may ask to be sponsored, rather than growing a second authorisation
//! system that has to agree with the first.
//!
//! # What the relay does and does not check
//!
//! The relay validates the event signature and membership. It does **not** check
//! the SNIP-9 signature inside the payload — that is the account's job, on chain.
//! So a stored request means "a member asked", never "this will succeed".

use serde::{Deserialize, Serialize};

/// Errors parsing or validating a sponsorship payload.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SponsorshipError {
    /// The content was not the expected JSON shape.
    #[error("malformed sponsorship payload: {0}")]
    Malformed(String),
    /// A field was present but unusable.
    #[error("invalid {field}: {reason}")]
    Invalid {
        /// Which field.
        field: &'static str,
        /// Why it was rejected.
        reason: String,
    },
}

/// One call a sponsored request wants executed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SponsorCall {
    /// Callee contract address, felt hex.
    pub to: String,
    /// Entry-point selector, felt hex.
    pub selector: String,
    /// Calldata, felt hex.
    #[serde(default)]
    pub calldata: Vec<String>,
}

/// A request for the paymaster to pay for and submit a SNIP-9 execution.
///
/// The signature is over the SNIP-12 hash of the outside execution, which binds
/// the account address and chain id. So a request cannot be replayed against a
/// different account or a different network even though this envelope does not
/// repeat those fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SponsorRequest {
    /// Starknet chain id short string, e.g. `SN_MAIN`.
    pub chain_id: String,
    /// SNIP-9 `caller` — a relayer address, or `ANY_CALLER`.
    pub caller: String,
    /// Single-use nonce. Must equal the event's `d` tag so replacement is
    /// idempotent.
    pub nonce: String,
    /// Valid strictly after this Unix timestamp.
    pub execute_after: u64,
    /// Valid strictly before this Unix timestamp.
    pub execute_before: u64,
    /// The calls to run, all-or-nothing.
    pub calls: Vec<SponsorCall>,
    /// BIP-340 signature as `[r_low, r_high, s_low, s_high]`, felt hex.
    pub signature: Vec<String>,
}

impl SponsorRequest {
    /// Parses and validates a request from event content.
    pub fn from_json(content: &str) -> Result<Self, SponsorshipError> {
        let parsed: Self = serde_json::from_str(content)
            .map_err(|e| SponsorshipError::Malformed(e.to_string()))?;
        parsed.validate()?;
        Ok(parsed)
    }

    /// Rejects requests that cannot possibly succeed on chain.
    ///
    /// Every check here mirrors one the account or the SNIP-9 component enforces.
    /// Doing it before submission is what keeps a malformed request from costing
    /// the sponsor a fee to discover — the sponsor pays for failed transactions
    /// too.
    pub fn validate(&self) -> Result<(), SponsorshipError> {
        if self.chain_id.is_empty() || self.chain_id.len() > 31 {
            return Err(SponsorshipError::Invalid {
                field: "chain_id",
                reason: "must be a non-empty Cairo short string".into(),
            });
        }
        if self.signature.len() != 4 {
            // The account rejects any other length outright, so this would be a
            // guaranteed-failing transaction.
            return Err(SponsorshipError::Invalid {
                field: "signature",
                reason: format!(
                    "expected 4 felts [r_low,r_high,s_low,s_high], got {}",
                    self.signature.len()
                ),
            });
        }
        if self.calls.is_empty() {
            return Err(SponsorshipError::Invalid {
                field: "calls",
                reason: "a sponsored execution with no calls would pay a fee to do nothing".into(),
            });
        }
        if self.execute_after >= self.execute_before {
            return Err(SponsorshipError::Invalid {
                field: "execute_before",
                reason: format!(
                    "window is empty: after {} is not before {}",
                    self.execute_after, self.execute_before
                ),
            });
        }
        if self.nonce.is_empty() {
            return Err(SponsorshipError::Invalid {
                field: "nonce",
                reason: "required, and must match the event d tag".into(),
            });
        }
        Ok(())
    }

    /// Whether the request's nonce agrees with the event's `d` tag.
    ///
    /// They must match or idempotency breaks: replacement is keyed on the d tag,
    /// while replay protection on chain is keyed on the nonce. A mismatch lets two
    /// distinct requests occupy one replaceable slot, or one nonce span two slots.
    pub fn nonce_matches_d_tag(&self, d_tag: &str) -> bool {
        self.nonce == d_tag
    }
}

/// What the paymaster did with a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SponsorResult {
    /// Submitted. `transaction_hash` identifies it on chain.
    Submitted {
        /// The nonce of the request this answers.
        nonce: String,
        /// Starknet transaction hash, felt hex.
        transaction_hash: String,
        /// Whether the account was deployed as part of the same transaction.
        deployed: bool,
    },
    /// Not submitted, with a reason.
    ///
    /// Deliberately a flat string rather than a typed enum: the reasons are for a
    /// human reading a client, and a closed set would need a wire change every time
    /// a new refusal appears.
    Declined {
        /// The nonce of the request this answers.
        nonce: String,
        /// Why.
        reason: String,
    },
}

impl SponsorResult {
    /// The request nonce this result answers.
    pub fn nonce(&self) -> &str {
        match self {
            SponsorResult::Submitted { nonce, .. } | SponsorResult::Declined { nonce, .. } => nonce,
        }
    }

    /// Serialises to event content.
    pub fn to_json(&self) -> String {
        // The shapes here are plain data with no borrowed lifetimes or maps, so
        // serialisation cannot fail; expressing that as a panic-free fallback keeps
        // the signature honest without an unwrap in a production path.
        serde_json::to_string(self).unwrap_or_else(|_| {
            r#"{"status":"declined","nonce":"","reason":"result serialisation failed"}"#.to_string()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> SponsorRequest {
        SponsorRequest {
            chain_id: "SN_MAIN".into(),
            caller: "ANY_CALLER".into(),
            nonce: "0x2a".into(),
            execute_after: 1_000,
            execute_before: 2_000,
            calls: vec![SponsorCall {
                to: "0x1234".into(),
                selector: "0x5678".into(),
                calldata: vec!["0x1".into()],
            }],
            signature: vec!["0x1".into(), "0x2".into(), "0x3".into(), "0x4".into()],
        }
    }

    #[test]
    fn a_valid_request_round_trips() {
        let json = serde_json::to_string(&valid()).unwrap();
        assert_eq!(SponsorRequest::from_json(&json).unwrap(), valid());
    }

    #[test]
    fn a_wrong_length_signature_is_rejected_before_it_costs_a_fee() {
        // The account rejects anything but 4 felts, so submitting would be a
        // guaranteed failure the sponsor still pays for.
        let mut r = valid();
        r.signature = vec!["0x1".into(), "0x2".into()];
        assert!(matches!(
            r.validate(),
            Err(SponsorshipError::Invalid {
                field: "signature",
                ..
            })
        ));
    }

    #[test]
    fn an_empty_call_list_is_rejected() {
        let mut r = valid();
        r.calls.clear();
        assert!(matches!(
            r.validate(),
            Err(SponsorshipError::Invalid { field: "calls", .. })
        ));
    }

    #[test]
    fn an_inverted_window_is_rejected() {
        let mut r = valid();
        r.execute_before = r.execute_after;
        assert!(matches!(
            r.validate(),
            Err(SponsorshipError::Invalid {
                field: "execute_before",
                ..
            })
        ));
    }

    #[test]
    fn an_over_long_chain_id_is_rejected() {
        let mut r = valid();
        r.chain_id = "A".repeat(32);
        assert!(matches!(
            r.validate(),
            Err(SponsorshipError::Invalid {
                field: "chain_id",
                ..
            })
        ));
    }

    #[test]
    fn a_missing_nonce_is_rejected() {
        let mut r = valid();
        r.nonce.clear();
        assert!(matches!(
            r.validate(),
            Err(SponsorshipError::Invalid { field: "nonce", .. })
        ));
    }

    #[test]
    fn garbage_content_is_malformed_not_a_panic() {
        assert!(matches!(
            SponsorRequest::from_json("not json"),
            Err(SponsorshipError::Malformed(_))
        ));
        // A stale NIP-SW wallet binding still stored at kind 30900 lands here:
        // valid JSON, wrong shape. It must be ignored, not misread.
        assert!(matches!(
            SponsorRequest::from_json(r#"{"chain_id":"SN_MAIN","address":"0x1"}"#),
            Err(SponsorshipError::Malformed(_))
        ));
    }

    #[test]
    fn the_nonce_must_match_the_d_tag() {
        // Idempotency is keyed on the d tag, replay protection on the nonce. If
        // they disagree, two requests can share one replaceable slot.
        assert!(valid().nonce_matches_d_tag("0x2a"));
        assert!(!valid().nonce_matches_d_tag("0x2b"));
    }

    #[test]
    fn results_carry_the_nonce_they_answer() {
        let s = SponsorResult::Submitted {
            nonce: "0x2a".into(),
            transaction_hash: "0xabc".into(),
            deployed: true,
        };
        assert_eq!(s.nonce(), "0x2a");
        assert!(s.to_json().contains("\"status\":\"submitted\""));

        let d = SponsorResult::Declined {
            nonce: "0x2a".into(),
            reason: "no".into(),
        };
        assert_eq!(d.nonce(), "0x2a");
        assert!(d.to_json().contains("\"status\":\"declined\""));
    }

    #[test]
    fn results_round_trip_through_json() {
        let s = SponsorResult::Submitted {
            nonce: "0x2a".into(),
            transaction_hash: "0xabc".into(),
            deployed: false,
        };
        assert_eq!(
            serde_json::from_str::<SponsorResult>(&s.to_json()).unwrap(),
            s
        );
    }
}
