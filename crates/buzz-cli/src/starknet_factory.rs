//! `AccountFactory` for Nostr-key-controlled Starknet accounts.
//!
//! Bridges `contracts/src/account.cairo` to `starknet-accounts`, so the
//! maintained implementation builds the `DEPLOY_ACCOUNT` v3 transaction hash —
//! resource bounds, tip, nonce, DA mode, chain id — and this module only supplies
//! the signature.
//!
//! Hand-rolling that hash would be the wrong kind of ambitious: it is
//! consensus-critical, versioned, and has no test vectors to check against. The
//! signature, by contrast, *is* checkable, and is verified against the published
//! BIP-340 vectors in `buzz_core::starknet_account`.
//!
//! # Why this can't use the standard `Signer` trait
//!
//! `starknet-signers::Signer` returns a Stark-curve `Signature { r, s }` — two
//! felts. A BIP-340 signature needs four, because each 256-bit scalar is split
//! across two felts. `AccountFactory::sign_deployment_v3` returns `Vec<Felt>`,
//! which is why implementing the factory works where implementing a signer would
//! not.

use async_trait::async_trait;
use buzz_core::starknet_account::sign_tx_hash;
use starknet_accounts::{AccountFactory, PreparedAccountDeploymentV3, RawAccountDeploymentV3};
use starknet_core::types::{BlockId, BlockTag, Felt};
use starknet_providers::Provider;

/// Signing never fails: BIP-340 signing of a fixed-size hash has no error path
/// once the key is valid, and the key is validated when the factory is built.
#[derive(Debug, thiserror::Error)]
#[error("infallible")]
pub struct Infallible;

/// Deploys accounts whose owner is a Nostr key.
pub struct NostrAccountFactory<P> {
    class_hash: Felt,
    calldata: Vec<Felt>,
    chain_id: Felt,
    provider: P,
    secret_key: secp256k1::SecretKey,
}

impl<P> NostrAccountFactory<P> {
    /// Builds a factory for the account owned by `secret_key`'s public half.
    ///
    /// `calldata` must be `buzz_core::starknet_account::constructor_calldata` for
    /// the matching pubkey — the address is a hash of it, so a mismatch deploys
    /// to an address nobody derived.
    pub const fn new(
        class_hash: Felt,
        calldata: Vec<Felt>,
        chain_id: Felt,
        provider: P,
        secret_key: secp256k1::SecretKey,
    ) -> Self {
        Self {
            class_hash,
            calldata,
            chain_id,
            provider,
            secret_key,
        }
    }
}

#[async_trait]
impl<P> AccountFactory for NostrAccountFactory<P>
where
    P: Provider + Sync + Send,
{
    type Provider = P;
    type SignError = Infallible;

    fn class_hash(&self) -> Felt {
        self.class_hash
    }

    fn calldata(&self) -> Vec<Felt> {
        self.calldata.clone()
    }

    fn chain_id(&self) -> Felt {
        self.chain_id
    }

    fn provider(&self) -> &Self::Provider {
        &self.provider
    }

    /// Signing is local and cheap, so estimation may request real signatures.
    fn is_signer_interactive(&self) -> bool {
        false
    }

    fn block_id(&self) -> BlockId {
        BlockId::Tag(BlockTag::Latest)
    }

    async fn sign_deployment_v3(
        &self,
        deployment: &RawAccountDeploymentV3,
        query_only: bool,
    ) -> Result<Vec<Felt>, Self::SignError> {
        // `query_only` builds a hash that a state-changing transaction cannot
        // reuse, so an estimate signature can never be replayed as a real
        // deployment. Passed straight through rather than second-guessed.
        let tx_hash = PreparedAccountDeploymentV3::from_raw(deployment.clone(), self)
            .transaction_hash(query_only);
        Ok(sign_tx_hash(&self.secret_key, tx_hash).to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_core::starknet_account::{account_address, constructor_calldata};

    const PUBKEY: &str = "8dae5a92916c512029ad1534fcf264e0e2e33ce492acf34588bc6268f7570dd5";

    #[test]
    fn factory_calldata_matches_the_address_derivation_input() {
        // The factory's calldata and the derived address must come from the same
        // encoding. If they diverge, `deploy` sends to one address while
        // `wallet address` prints another, and funds go to the one nobody
        // deploys to.
        let calldata = constructor_calldata(PUBKEY).expect("calldata");
        let class_hash = Felt::from_hex("0x1234").expect("class hash");

        // Same inputs the factory would report.
        let derived = account_address(class_hash, PUBKEY).expect("derive");

        // Recompute through starknet-core the way starknet-accounts does.
        let independent = starknet_core::utils::get_contract_address(
            Felt::ZERO,
            class_hash,
            &calldata,
            Felt::ZERO,
        );
        assert_eq!(derived, independent);
    }

    #[test]
    fn signature_is_four_felts() {
        // The account rejects any other length, so the factory must emit exactly
        // four — this is the contract between Rust and Cairo.
        let key = secp256k1::SecretKey::from_byte_array([
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 3,
        ])
        .expect("published bip340 vector key");
        let felts = sign_tx_hash(&key, Felt::from_hex("0xabc").expect("hash"));
        assert_eq!(felts.len(), 4);
    }
}
