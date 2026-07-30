//! NIP-SW Starknet wallet bindings — parsing and validation primitives.
//!
//! See `docs/nips/NIP-SW.md` for the spec. This module is I/O-free: it performs
//! no on-chain verification and makes no network calls. It validates the shape
//! and internal consistency of a binding event so that the caller can then
//! verify the attestation against a Starknet RPC endpoint.
//!
//! The split matters: a binding that fails validation here can be rejected
//! without an RPC round trip, and this crate stays free of I/O dependencies.
//!
//! This module never handles private key material. The attestation is a
//! signature produced elsewhere by the user's own Starknet account; nothing
//! here signs, derives, or stores a secret.

use serde::{Deserialize, Serialize};

use crate::kind::KIND_STARKNET_WALLET_BINDING;

/// Prefix for the NIP-39 style `i` tag identity string.
pub const I_TAG_PLATFORM: &str = "starknet";

/// Maximum length of a felt-encoded ASCII short string (e.g. a chain id).
///
/// A felt252 holds at most 31 bytes of ASCII.
pub const CHAIN_ID_MAX_LEN: usize = 31;

/// Maximum number of hex digits in a felt252, excluding the `0x` prefix.
pub const FELT_HEX_MAX_DIGITS: usize = 64;

/// Maximum number of felts accepted in an attestation signature.
///
/// Generous enough for multisig accounts while bounding the work an unverified
/// submission can ask the relay to do.
pub const SIGNATURE_MAX_FELTS: usize = 32;

/// Errors from parsing or validating a wallet binding.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WalletBindingError {
    /// Event kind was not [`KIND_STARKNET_WALLET_BINDING`].
    #[error("wrong kind: expected {KIND_STARKNET_WALLET_BINDING}, got {0}")]
    WrongKind(u32),
    /// Content was not valid JSON, or did not match the payload schema.
    #[error("invalid payload: {0}")]
    InvalidPayload(String),
    /// A felt-hex field was malformed.
    #[error("invalid felt hex in {field}: {reason}")]
    InvalidFelt {
        /// Which payload field failed.
        field: &'static str,
        /// Why it failed.
        reason: String,
    },
    /// The chain id was empty or too long to be a felt short string.
    #[error("invalid chain id: {0}")]
    InvalidChainId(String),
    /// The event had no `d` tag, or more than one.
    #[error("expected exactly one `d` tag, found {0}")]
    DTagCount(usize),
    /// The event had no `i` tag, or more than one.
    #[error("expected exactly one `i` tag, found {0}")]
    ITagCount(usize),
    /// The `i` tag did not parse as `starknet:<chain_id>:<address>`.
    #[error("malformed `i` tag: {0}")]
    MalformedITag(String),
    /// Two fields that must agree disagreed. This is the spoofing-relevant
    /// check: a payload whose `i` tag and content name different addresses
    /// could be verified against one account while displayed as another.
    #[error("{field} mismatch: {a} != {b}")]
    Mismatch {
        /// Which value disagreed.
        field: &'static str,
        /// First occurrence.
        a: String,
        /// Second occurrence.
        b: String,
    },
    /// The attestation signature was empty or implausibly long.
    #[error("invalid signature: {0}")]
    InvalidSignature(String),
}

/// Signature scheme the bound account validates with.
///
/// Informational: verification calls `is_valid_signature` on the account, so
/// the relay does not branch on this. Clients may use it to warn about account
/// implementations they do not recognise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SignerScheme {
    /// Native Starknet curve.
    Stark,
    /// secp256k1 ECDSA — note this is *not* the BIP-340 Schnorr scheme Nostr
    /// signs with, even though the curve is the same.
    Secp256k1Ecdsa,
    /// secp256r1 (P-256) ECDSA, as used by passkey-backed accounts.
    Secp256r1,
    /// An account implementation this build does not know about.
    #[serde(other)]
    Unknown,
}

/// How the attestation was produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttestationScheme {
    /// SNIP-12 typed data.
    Snip12,
    /// An unrecognised scheme. Rejected during validation.
    #[serde(other)]
    Unknown,
}

/// Starknet-side proof that the account controls this binding.
///
/// Deliberately carries **no message hash**. The hash the account signed is
/// derived by the verifier from the binding's own fields via
/// [`crate::snip12::BindingMessage`], never taken from the submitter.
///
/// Accepting a submitted hash would be forgeable with public data alone:
/// Starknet transaction signatures are on-chain, so an attacker could lift any
/// `(tx_hash, signature)` pair from a victim's account and publish it as an
/// attestation under their own Nostr identity. The account would confirm the
/// signature is valid — because it is — over a message that says nothing about
/// the binding. Deriving the hash removes the field there was to spoof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attestation {
    /// Typed-data scheme the signature was produced under.
    pub scheme: AttestationScheme,
    /// Signature felts, as produced by the account's signer.
    pub signature: Vec<String>,
    /// Unix seconds when the attestation was produced.
    ///
    /// Surfaced to clients because an attestation proves control *at signing
    /// time* — Starknet accounts are upgradeable and owner-rotatable, so age
    /// is security-relevant, not cosmetic.
    pub signed_at: u64,
}

/// A parsed, internally consistent NIP-SW binding payload.
///
/// Construction via [`WalletBinding::parse`] guarantees shape and consistency
/// only. It does **not** mean the attestation is valid — that requires an
/// on-chain `is_valid_signature` call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletBinding {
    /// Account contract address, felt hex.
    pub address: String,
    /// Starknet chain id short string, e.g. `SN_MAIN`.
    pub chain_id: String,
    /// Account contract class hash, felt hex. Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class_hash: Option<String>,
    /// Signer scheme the account validates with.
    pub signer_scheme: SignerScheme,
    /// Starknet-side proof of control.
    pub attestation: Attestation,
}

/// Validate a felt-hex string: `0x` followed by 1..=64 hex digits.
fn validate_felt_hex(field: &'static str, value: &str) -> Result<(), WalletBindingError> {
    let Some(digits) = value.strip_prefix("0x") else {
        return Err(WalletBindingError::InvalidFelt {
            field,
            reason: "missing 0x prefix".into(),
        });
    };
    if digits.is_empty() {
        return Err(WalletBindingError::InvalidFelt {
            field,
            reason: "no hex digits".into(),
        });
    }
    if digits.len() > FELT_HEX_MAX_DIGITS {
        return Err(WalletBindingError::InvalidFelt {
            field,
            reason: format!("{} digits exceeds felt252 width", digits.len()),
        });
    }
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(WalletBindingError::InvalidFelt {
            field,
            reason: "non-hex digit".into(),
        });
    }
    Ok(())
}

/// Validate a chain id as a felt-encodable ASCII short string.
fn validate_chain_id(chain_id: &str) -> Result<(), WalletBindingError> {
    if chain_id.is_empty() {
        return Err(WalletBindingError::InvalidChainId("empty".into()));
    }
    if chain_id.len() > CHAIN_ID_MAX_LEN {
        return Err(WalletBindingError::InvalidChainId(format!(
            "{} bytes exceeds {CHAIN_ID_MAX_LEN}-byte short string limit",
            chain_id.len()
        )));
    }
    // Restrict to the short-string alphabet actually used by chain ids. This
    // keeps the `i` tag unambiguous: a chain id containing `:` would make
    // `starknet:<chain>:<address>` impossible to parse back.
    if !chain_id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(WalletBindingError::InvalidChainId(
            "expected ASCII alphanumeric, `_`, or `-`".into(),
        ));
    }
    Ok(())
}

/// Build the NIP-39 style `i` tag identity string for a binding.
///
/// Format is `starknet:<chain_id>:<address>`. Used both when publishing and
/// when constructing a reverse-lookup filter (`#i`).
#[must_use]
pub fn i_tag_value(chain_id: &str, address: &str) -> String {
    format!("{I_TAG_PLATFORM}:{chain_id}:{address}")
}

/// The `(chain_id, address)` pair carried by an `i` tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ITagIdentity {
    /// Chain id short string.
    pub chain_id: String,
    /// Account address, felt hex.
    pub address: String,
}

/// Parse a NIP-39 style `i` tag identity string.
///
/// Splits on the first two `:` only, so an address containing `:` is rejected
/// by felt validation rather than silently truncated.
pub fn parse_i_tag_value(value: &str) -> Result<ITagIdentity, WalletBindingError> {
    let mut parts = value.splitn(3, ':');
    let platform = parts
        .next()
        .ok_or_else(|| WalletBindingError::MalformedITag("empty".into()))?;
    if platform != I_TAG_PLATFORM {
        return Err(WalletBindingError::MalformedITag(format!(
            "expected platform `{I_TAG_PLATFORM}`, got `{platform}`"
        )));
    }
    let chain_id = parts
        .next()
        .ok_or_else(|| WalletBindingError::MalformedITag("missing chain id".into()))?;
    let address = parts
        .next()
        .ok_or_else(|| WalletBindingError::MalformedITag("missing address".into()))?;
    validate_chain_id(chain_id)?;
    validate_felt_hex("i tag address", address)?;
    Ok(ITagIdentity {
        chain_id: chain_id.to_string(),
        address: address.to_string(),
    })
}

impl WalletBinding {
    /// Parse and validate a payload from event content.
    ///
    /// Checks JSON shape, felt-hex formats, chain id encodability, attestation
    /// scheme, and signature bounds. Does not check the `d`/`i` tags — use
    /// [`WalletBinding::from_event`] for a whole event.
    pub fn parse(content: &str) -> Result<Self, WalletBindingError> {
        let binding: Self = serde_json::from_str(content)
            .map_err(|e| WalletBindingError::InvalidPayload(e.to_string()))?;
        binding.validate()?;
        Ok(binding)
    }

    /// Validate an already-deserialized payload.
    pub fn validate(&self) -> Result<(), WalletBindingError> {
        validate_felt_hex("address", &self.address)?;
        validate_chain_id(&self.chain_id)?;
        if let Some(class_hash) = &self.class_hash {
            validate_felt_hex("class_hash", class_hash)?;
        }
        if self.attestation.scheme == AttestationScheme::Unknown {
            return Err(WalletBindingError::InvalidPayload(
                "unrecognised attestation scheme".into(),
            ));
        }
        if self.attestation.signature.is_empty() {
            return Err(WalletBindingError::InvalidSignature("empty".into()));
        }
        if self.attestation.signature.len() > SIGNATURE_MAX_FELTS {
            return Err(WalletBindingError::InvalidSignature(format!(
                "{} felts exceeds maximum {SIGNATURE_MAX_FELTS}",
                self.attestation.signature.len()
            )));
        }
        for felt in &self.attestation.signature {
            validate_felt_hex("attestation.signature", felt)?;
        }
        Ok(())
    }

    /// Parse and validate a whole binding event, including tag consistency.
    ///
    /// Enforces exactly one `d` tag equal to the payload `chain_id`, and
    /// exactly one `i` tag whose chain id and address both match the payload.
    /// A mismatch here is the spoofing case the checks exist for: a client
    /// reading the `i` tag and a relay verifying the payload address must never
    /// be looking at different accounts.
    pub fn from_event(event: &nostr::Event) -> Result<Self, WalletBindingError> {
        let kind_u32 = u32::from(event.kind.as_u16());
        if kind_u32 != KIND_STARKNET_WALLET_BINDING {
            return Err(WalletBindingError::WrongKind(kind_u32));
        }

        let binding = Self::parse(&event.content)?;

        let d_values = tag_values(event, nostr::Alphabet::D);
        let [d_tag] = d_values.as_slice() else {
            return Err(WalletBindingError::DTagCount(d_values.len()));
        };
        if d_tag != &binding.chain_id {
            return Err(WalletBindingError::Mismatch {
                field: "chain_id (d tag vs payload)",
                a: d_tag.clone(),
                b: binding.chain_id.clone(),
            });
        }

        let i_values = tag_values(event, nostr::Alphabet::I);
        let [i_tag] = i_values.as_slice() else {
            return Err(WalletBindingError::ITagCount(i_values.len()));
        };
        let identity = parse_i_tag_value(i_tag)?;
        if identity.chain_id != binding.chain_id {
            return Err(WalletBindingError::Mismatch {
                field: "chain_id (i tag vs payload)",
                a: identity.chain_id,
                b: binding.chain_id.clone(),
            });
        }
        if !addresses_equal(&identity.address, &binding.address) {
            return Err(WalletBindingError::Mismatch {
                field: "address (i tag vs payload)",
                a: identity.address,
                b: binding.address.clone(),
            });
        }

        Ok(binding)
    }
}

/// Collect the first value of every tag with the given single-letter key.
fn tag_values(event: &nostr::Event, alphabet: nostr::Alphabet) -> Vec<String> {
    let key = nostr::SingleLetterTag::lowercase(alphabet);
    event
        .tags
        .filter(nostr::TagKind::SingleLetter(key))
        .filter_map(|t| t.content().map(std::string::ToString::to_string))
        .collect()
}

/// Compare two felt-hex addresses for numeric equality.
///
/// Starknet addresses are routinely written with and without leading zeros
/// (`0x04a5…` vs `0x4a5…`) and in either case. A naive string comparison would
/// reject a correct binding, so normalise before comparing. Assumes both sides
/// already passed [`validate_felt_hex`].
fn addresses_equal(a: &str, b: &str) -> bool {
    fn normalise(s: &str) -> String {
        s.strip_prefix("0x")
            .unwrap_or(s)
            .trim_start_matches('0')
            .to_ascii_lowercase()
    }
    normalise(a) == normalise(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attestation() -> Attestation {
        Attestation {
            scheme: AttestationScheme::Snip12,
            signature: vec!["0xaa".into(), "0xbb".into()],
            signed_at: 1_785_400_000,
        }
    }

    fn binding() -> WalletBinding {
        WalletBinding {
            address: "0x04a5".into(),
            chain_id: "SN_MAIN".into(),
            class_hash: Some("0x02b3".into()),
            signer_scheme: SignerScheme::Secp256k1Ecdsa,
            attestation: attestation(),
        }
    }

    #[test]
    fn round_trips_through_json() {
        let json = serde_json::to_string(&binding()).expect("serialize");
        let parsed = WalletBinding::parse(&json).expect("parse");
        assert_eq!(parsed, binding());
    }

    #[test]
    fn class_hash_is_optional() {
        let mut b = binding();
        b.class_hash = None;
        let json = serde_json::to_string(&b).expect("serialize");
        assert!(!json.contains("class_hash"));
        assert_eq!(WalletBinding::parse(&json).expect("parse"), b);
    }

    #[test]
    fn rejects_felt_without_prefix() {
        let mut b = binding();
        b.address = "04a5".into();
        assert!(matches!(
            b.validate(),
            Err(WalletBindingError::InvalidFelt {
                field: "address",
                ..
            })
        ));
    }

    #[test]
    fn rejects_non_hex_felt() {
        let mut b = binding();
        b.address = "0xzz".into();
        assert!(matches!(
            b.validate(),
            Err(WalletBindingError::InvalidFelt {
                field: "address",
                ..
            })
        ));
    }

    #[test]
    fn rejects_overlong_felt() {
        let mut b = binding();
        b.address = format!("0x{}", "a".repeat(FELT_HEX_MAX_DIGITS + 1));
        assert!(matches!(
            b.validate(),
            Err(WalletBindingError::InvalidFelt {
                field: "address",
                ..
            })
        ));
    }

    #[test]
    fn rejects_empty_signature() {
        let mut b = binding();
        b.attestation.signature.clear();
        assert!(matches!(
            b.validate(),
            Err(WalletBindingError::InvalidSignature(_))
        ));
    }

    #[test]
    fn rejects_oversized_signature() {
        let mut b = binding();
        b.attestation.signature = vec!["0x1".into(); SIGNATURE_MAX_FELTS + 1];
        assert!(matches!(
            b.validate(),
            Err(WalletBindingError::InvalidSignature(_))
        ));
    }

    #[test]
    fn rejects_unknown_attestation_scheme() {
        let json = r#"{
            "address":"0x04a5","chain_id":"SN_MAIN","signer_scheme":"stark",
            "attestation":{"scheme":"handwave","signature":["0x2"],"signed_at":1}
        }"#;
        assert!(matches!(
            WalletBinding::parse(json),
            Err(WalletBindingError::InvalidPayload(_))
        ));
    }

    #[test]
    fn unknown_signer_scheme_parses_but_is_marked_unknown() {
        // Forward compatibility: a new account implementation must not make the
        // binding unparseable, only unrecognised.
        let json = r#"{
            "address":"0x04a5","chain_id":"SN_MAIN","signer_scheme":"future-curve",
            "attestation":{"scheme":"snip12","signature":["0x2"],"signed_at":1}
        }"#;
        let parsed = WalletBinding::parse(json).expect("parse");
        assert_eq!(parsed.signer_scheme, SignerScheme::Unknown);
    }

    #[test]
    fn rejects_chain_id_with_colon() {
        // A colon would make the `i` tag ambiguous to parse back.
        let mut b = binding();
        b.chain_id = "SN:MAIN".into();
        assert!(matches!(
            b.validate(),
            Err(WalletBindingError::InvalidChainId(_))
        ));
    }

    #[test]
    fn rejects_overlong_chain_id() {
        let mut b = binding();
        b.chain_id = "A".repeat(CHAIN_ID_MAX_LEN + 1);
        assert!(matches!(
            b.validate(),
            Err(WalletBindingError::InvalidChainId(_))
        ));
    }

    #[test]
    fn i_tag_round_trips() {
        let value = i_tag_value("SN_MAIN", "0x04a5");
        assert_eq!(value, "starknet:SN_MAIN:0x04a5");
        let parsed = parse_i_tag_value(&value).expect("parse");
        assert_eq!(parsed.chain_id, "SN_MAIN");
        assert_eq!(parsed.address, "0x04a5");
    }

    #[test]
    fn rejects_i_tag_wrong_platform() {
        assert!(matches!(
            parse_i_tag_value("ethereum:1:0x04a5"),
            Err(WalletBindingError::MalformedITag(_))
        ));
    }

    #[test]
    fn rejects_i_tag_missing_address() {
        assert!(matches!(
            parse_i_tag_value("starknet:SN_MAIN"),
            Err(WalletBindingError::MalformedITag(_))
        ));
    }

    #[test]
    fn addresses_compare_numerically_not_textually() {
        // Leading zeros and case are both cosmetic in felt hex; rejecting a
        // correct binding over formatting would be a real bug.
        assert!(addresses_equal("0x04a5", "0x4A5"));
        assert!(addresses_equal("0x0", "0x000"));
        assert!(!addresses_equal("0x04a5", "0x04a6"));
    }
}
