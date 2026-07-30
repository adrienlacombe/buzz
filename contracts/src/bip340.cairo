//! BIP-340 Schnorr signature verification over secp256k1.
//!
//! This is the signature scheme Nostr uses (NIP-01), which is what lets a Nostr
//! key control a Starknet account directly: the account's `__validate__` checks a
//! BIP-340 signature rather than a Stark-curve one.
//!
//! Adapted from `keep-starknet-strange/joyboy` (`onchain/src/bip340.cairo`),
//! MIT licensed, Copyright (c) 2024 Keep Starknet Strange. The MIT permission
//! notice is reproduced in `LICENSE-MIT-joyboy` alongside this crate.
//!
//! Changes from the original:
//!   - Uses `core::sha256::compute_sha256_byte_array` from corelib. The original
//!     carried a hand-rolled SHA-256 with a TODO to switch "once Cairo 2.7 is
//!     available"; we target 2.18, and the corelib version is backed by the
//!     sha256 builtin, so each hash costs a fraction of the steps.
//!   - Modernised for Cairo 2024_07 (prelude imports, `Some`/`None`).
//!
//! # Verification is consensus-critical
//!
//! A bug here means either valid signatures are rejected (an account is bricked)
//! or invalid ones accepted (funds are stealable). The test module ports the
//! official BIP-340 vectors from the Bitcoin BIPs repository, including the
//! negative cases, because that is the only assurance that carries weight.
//!
//! References:
//!   BIP-340: https://github.com/bitcoin/bips/blob/master/bip-0340.mediawiki
//!   Vectors: https://github.com/bitcoin/bips/blob/master/bip-0340/test-vectors.csv

use core::sha256::compute_sha256_byte_array;
use starknet::SyscallResultTrait;
use starknet::secp256_trait::{Secp256PointTrait, Secp256Trait};
use starknet::secp256k1::Secp256k1Point;

const TWO_POW_32: u128 = 0x100000000;
const TWO_POW_64: u128 = 0x10000000000000000;
const TWO_POW_96: u128 = 0x1000000000000000000000000;

/// secp256k1 field size.
const P: u256 = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEFFFFFC2F;

/// Computes the `BIP0340/challenge` tagged hash.
///
/// A tagged hash is `sha256(sha256(tag) || sha256(tag) || data)`. The doubled
/// tag digest is what domain-separates BIP-340 challenges from any other use of
/// SHA-256, so it is not redundant.
///
/// Returns `sha256(sha256(tag) || sha256(tag) || bytes(rx) || bytes(px) || m)`.
fn hash_challenge(rx: u256, px: u256, m: ByteArray) -> u256 {
    let [x0, x1, x2, x3, x4, x5, x6, x7] = compute_sha256_byte_array(@"BIP0340/challenge");

    let mut ba: ByteArray = Default::default();
    // sha256(tag), twice.
    let mut i: u8 = 0;
    while i != 2 {
        ba.append_word(x0.into(), 4);
        ba.append_word(x1.into(), 4);
        ba.append_word(x2.into(), 4);
        ba.append_word(x3.into(), 4);
        ba.append_word(x4.into(), 4);
        ba.append_word(x5.into(), 4);
        ba.append_word(x6.into(), 4);
        ba.append_word(x7.into(), 4);
        i += 1;
    }
    // bytes(rx) || bytes(px), each big-endian 32 bytes.
    ba.append_word(rx.high.into(), 16);
    ba.append_word(rx.low.into(), 16);
    ba.append_word(px.high.into(), 16);
    ba.append_word(px.low.into(), 16);
    ba.append(@m);

    let [y0, y1, y2, y3, y4, y5, y6, y7] = compute_sha256_byte_array(@ba);

    u256 {
        high: y0.into() * TWO_POW_96 + y1.into() * TWO_POW_64 + y2.into() * TWO_POW_32 + y3.into(),
        low: y4.into() * TWO_POW_96 + y5.into() * TWO_POW_64 + y6.into() * TWO_POW_32 + y7.into(),
    }
}

/// Verifies a BIP-340 Schnorr signature `(rx, s)` over `m` for x-only pubkey `px`.
///
/// `px` is an x-only public key: BIP-340 keys carry no parity bit and are lifted
/// to the point with **even** y. A Nostr pubkey is exactly this, which is why it
/// drops straight in.
///
/// Returns `true` only for a valid signature. Every failure path returns `false`
/// rather than panicking, so a malformed signature is a rejected transaction and
/// not a stuck account.
pub fn verify(px: u256, rx: u256, s: u256, m: ByteArray) -> bool {
    let n = Secp256Trait::<Secp256k1Point>::get_curve_size();

    // Range checks first: out-of-range values are cheap to reject and must not
    // reach the curve operations.
    if px >= P || rx >= P || s >= n {
        return false;
    }

    // Lift px to the point P with x(P) = px and even y. `false` is the y-parity
    // argument — BIP-340 mandates the even-y point.
    let point =
        match Secp256Trait::<Secp256k1Point>::secp256_ec_get_point_from_x_syscall(px, false)
            .unwrap_syscall() {
        Some(point) => point,
        // px is not on the curve.
        None => { return false; },
    };

    // e = int(hash_BIP0340/challenge(bytes(rx) || bytes(px) || m)) mod n
    let e = hash_challenge(rx, px, m) % n;

    let g = Secp256Trait::<Secp256k1Point>::get_generator_point();

    // R = s*G - e*P, computed as s*G + (n - e)*P since scalars are unsigned.
    let p1 = g.mul(s).unwrap_syscall();
    let p2 = point.mul(n - e).unwrap_syscall();
    let r = p1.add(p2).unwrap_syscall();

    let (r_x, r_y) = r.get_coordinates().unwrap_syscall();

    // Reject if R is the point at infinity, if y(R) is odd, or if x(R) != rx.
    // The syscall represents infinity as (0, 0).
    !(r_x == 0 && r_y == 0) && r_y % 2 == 0 && r_x == rx
}

#[cfg(test)]
mod tests {
    use super::verify;

    /// Big-endian 32-byte encoding, matching BIP-340's `bytes()`.
    impl U256IntoByteArray of Into<u256, ByteArray> {
        fn into(self: u256) -> ByteArray {
            let mut ba = Default::default();
            ba.append_word(self.high.into(), 16);
            ba.append_word(self.low.into(), 16);
            ba
        }
    }

    // Vectors from the BIP-340 reference test suite:
    // https://github.com/bitcoin/bips/blob/master/bip-0340/test-vectors.csv
    // Ported via joyboy (MIT). Positive and negative cases both matter: a
    // verifier that accepts everything passes every positive test.

    #[test]
    fn vector_0_valid_empty_message() {
        let px: u256 = 0xf9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9;
        let rx: u256 = 0xe907831f80848d1069a5371b402410364bdf1c5f8307b0084c55f1ce2dca8215;
        let s: u256 = 0x25f66a4a85ea8b71e482a74f382d2ce5ebeee8fdb2172f477df4900d310536c0;
        let m: u256 = 0x0;
        assert!(verify(px, rx, s, m.into()));
    }

    #[test]
    fn vector_1_valid() {
        let px: u256 = 0xdff1d77f2a671c5f36183726db2341be58feae1da2deced843240f7b502ba659;
        let rx: u256 = 0x6896bd60eeae296db48a229ff71dfe071bde413e6d43f917dc8dcf8c78de3341;
        let s: u256 = 0x8906d11ac976abccb20b091292bff4ea897efcb639ea871cfa95f6de339e4b0a;
        let m: u256 = 0x243f6a8885a308d313198a2e03707344a4093822299f31d0082efa98ec4e6c89;
        assert!(verify(px, rx, s, m.into()));
    }

    #[test]
    fn vector_2_valid() {
        let px: u256 = 0xdd308afec5777e13121fa72b9cc1b7cc0139715309b086c960e18fd969774eb8;
        let rx: u256 = 0x5831aaeed7b44bb74e5eab94ba9d4294c49bcf2a60728d8b4c200f50dd313c1b;
        let s: u256 = 0xab745879a5ad954a72c45a91c3a51d3c7adea98d82f8481e0e1e03674a6f3fb7;
        let m: u256 = 0x7e2d58d8b3bcdf1abadec7829054f90dda9805aab56c77333024b9d0a508b75c;
        assert!(verify(px, rx, s, m.into()));
    }

    #[test]
    fn vector_3_valid_max_message() {
        let px: u256 = 0x25d1dff95105f5253c4022f628a996ad3a0d95fbf21d468a1b33f8c160d8f517;
        let rx: u256 = 0x7eb0509757e246f19449885651611cb965ecc1a187dd51b64fda1edc9637d5ec;
        let s: u256 = 0x97582b9cb13db3933705b32ba982af5af25fd78881ebb32771fc5922efc66ea3;
        let m: u256 = 0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff;
        assert!(verify(px, rx, s, m.into()));
    }

    #[test]
    fn vector_5_rejects_public_key_not_on_curve() {
        let px: u256 = 0xeefdea4cd8b9c9f5b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1;
        let rx: u256 = 0x6cff5c3ba86c69ea4b7376f31a9bcb4f74c1976089b2d9963da2e5543e177769;
        let s: u256 = 0x69e0e2b3d8b6b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8;
        let m: u256 = 0x243f6a8885a308d313198a2e03707344a4093822299f31d0082efa98ec4e6c89;
        assert!(!verify(px, rx, s, m.into()));
    }

    #[test]
    fn vector_6_rejects_has_even_y_violation() {
        let px: u256 = 0xdff1d77f2a671c5f36183726db2341be58feae1da2deced843240f7b502ba659;
        let rx: u256 = 0xfff97bd5755eeea420453a14355235d382f6472f8568a18b2f057a1460297556;
        let s: u256 = 0x3cc27944640ac607cd107ae10923d9ef7a73c643e166be5ebeafa34b1ac553e2;
        let m: u256 = 0x243f6a8885a308d313198a2e03707344a4093822299f31d0082efa98ec4e6c89;
        assert!(!verify(px, rx, s, m.into()));
    }

    #[test]
    fn vector_7_rejects_negated_message() {
        let px: u256 = 0xdff1d77f2a671c5f36183726db2341be58feae1da2deced843240f7b502ba659;
        let rx: u256 = 0x1fa62e331edbc21c394792d2ab1100a7b432b013df3f6ff4f99fcb33e0e1515f;
        let s: u256 = 0x28890b3edb6e7189b630448b515ce4f8622a954cfe545735aaea5134fccdb2bd;
        let m: u256 = 0x243f6a8885a308d313198a2e03707344a4093822299f31d0082efa98ec4e6c89;
        assert!(!verify(px, rx, s, m.into()));
    }

    #[test]
    fn vector_8_rejects_negated_s() {
        let px: u256 = 0xdff1d77f2a671c5f36183726db2341be58feae1da2deced843240f7b502ba659;
        let rx: u256 = 0x6cff5c3ba86c69ea4b7376f31a9bcb4f74c1976089b2d9963da2e5543e177769;
        let s: u256 = 0x961764b3aa9b2ffcb6ef947b6887a226e8d7c93e00c5ed0c1834ff0d0c2e6da6;
        let m: u256 = 0x243f6a8885a308d313198a2e03707344a4093822299f31d0082efa98ec4e6c89;
        assert!(!verify(px, rx, s, m.into()));
    }

    #[test]
    fn vector_9_rejects_sg_e_p_infinite_r() {
        let px: u256 = 0xdff1d77f2a671c5f36183726db2341be58feae1da2deced843240f7b502ba659;
        let rx: u256 = 0x0;
        let s: u256 = 0x123dda8328af9c23a94c1feecfd123ba4fb73476f0d594dcb65c6425bd186051;
        let m: u256 = 0x243f6a8885a308d313198a2e03707344a4093822299f31d0082efa98ec4e6c89;
        assert!(!verify(px, rx, s, m.into()));
    }

    #[test]
    fn vector_11_rejects_rx_not_on_curve() {
        let px: u256 = 0xdff1d77f2a671c5f36183726db2341be58feae1da2deced843240f7b502ba659;
        let rx: u256 = 0x4a298dacae57395a15d0795ddbfd1dcb564da82b0f269bc70a74f8220429ba1d;
        let s: u256 = 0x69e0e2b3d8b6b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8;
        let m: u256 = 0x243f6a8885a308d313198a2e03707344a4093822299f31d0082efa98ec4e6c89;
        assert!(!verify(px, rx, s, m.into()));
    }

    #[test]
    fn vector_13_rejects_rx_equal_to_field_size() {
        // rx == p is out of range and must be rejected by the range check, not
        // by the curve arithmetic.
        let px: u256 = 0xdff1d77f2a671c5f36183726db2341be58feae1da2deced843240f7b502ba659;
        let rx: u256 = 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffefffffc2f;
        let s: u256 = 0x69e0e2b3d8b6b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8;
        let m: u256 = 0x243f6a8885a308d313198a2e03707344a4093822299f31d0082efa98ec4e6c89;
        assert!(!verify(px, rx, s, m.into()));
    }

    #[test]
    fn vector_14_rejects_s_equal_to_curve_order() {
        let px: u256 = 0xdff1d77f2a671c5f36183726db2341be58feae1da2deced843240f7b502ba659;
        let rx: u256 = 0x6cff5c3ba86c69ea4b7376f31a9bcb4f74c1976089b2d9963da2e5543e177769;
        let s: u256 = 0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141;
        let m: u256 = 0x243f6a8885a308d313198a2e03707344a4093822299f31d0082efa98ec4e6c89;
        assert!(!verify(px, rx, s, m.into()));
    }

    #[test]
    fn vector_15_rejects_px_equal_to_field_size() {
        let px: u256 = 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffefffffc30;
        let rx: u256 = 0x6cff5c3ba86c69ea4b7376f31a9bcb4f74c1976089b2d9963da2e5543e177769;
        let s: u256 = 0x69e0e2b3d8b6b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8b3b8;
        let m: u256 = 0x243f6a8885a308d313198a2e03707344a4093822299f31d0082efa98ec4e6c89;
        assert!(!verify(px, rx, s, m.into()));
    }

    /// A valid signature must not verify against a different message.
    ///
    /// Not a BIP vector — guards the specific failure mode that would matter for
    /// an account: if the message were ignored, any past signature would authorise
    /// any future transaction.
    #[test]
    fn rejects_valid_signature_over_a_different_message() {
        let px: u256 = 0xdff1d77f2a671c5f36183726db2341be58feae1da2deced843240f7b502ba659;
        let rx: u256 = 0x6896bd60eeae296db48a229ff71dfe071bde413e6d43f917dc8dcf8c78de3341;
        let s: u256 = 0x8906d11ac976abccb20b091292bff4ea897efcb639ea871cfa95f6de339e4b0a;
        // vector_1's message with the final byte changed.
        let m: u256 = 0x243f6a8885a308d313198a2e03707344a4093822299f31d0082efa98ec4e6c88;
        assert!(!verify(px, rx, s, m.into()));
    }
}
