# Deployments

## `NostrAccount`

A Starknet account whose owner is a Nostr x-only pubkey, validating BIP-340
Schnorr signatures. See [`src/account.cairo`](src/account.cairo).

> ## ⚠️ The declared class below is superseded and must be re-declared
>
> Adding SNIP-9 sponsored execution changed the contract, so the class hash moved:
>
> | | |
> |---|---|
> | Declared on mainnet | `0x038a57ffba543e9fd54998a60436effc14e878cdcc64d4676ff642396fda346e` |
> | Current source builds to | `0x0414f62ea1ed35f8c7bd3b794d94efc95e01bccf04e0f47211fc198f7f56f537` |
>
> **Do not derive addresses from the declared hash any more** — it belongs to a
> class with no `execute_from_outside_v2`, so accounts at those addresses could
> never be sponsored, and a fresh account cannot pay its own ~0.78 STRK of
> BIP-340 verification.
>
> Nothing was ever deployed, so this costs nothing beyond a re-declare. That is
> precisely why the change was made before the first deployment rather than after:
> the address is a hash of the class, so every address changes with it, and doing
> this later would have orphaned every existing account and any funds sent to one.
>
> Re-declare with `sncast`, then replace the hashes and the tx below and delete
> this notice. The measurements further down still hold — SNIP-9 adds entry points
> but does not touch the BIP-340 path that dominates the cost.

### Mainnet (`SN_MAIN`) — superseded, see the notice above

| | |
|---|---|
| Class hash | `0x038a57ffba543e9fd54998a60436effc14e878cdcc64d4676ff642396fda346e` |
| Declare tx | `0x04a5e5356cd64abd1fba1ce4a70e28e34ca993b3b25bb71d7b266be99c987d5a` |
| Status | `SUCCEEDED` / `ACCEPTED_ON_L2` |
| Declared with | `sncast --account snip36-e2e-bd1d4eaf` |
| Cairo / Scarb | 2.18.0 |

Verified after declaring: `starknet_getClass` returns the class with 7 external
entry points and 1 constructor, and the on-chain class hash equals the one
`buzz wallet class-hash` computes from `target/dev/`. Local and sequencer agree.

Voyager: <https://voyager.online/class/0x038a57ffba543e9fd54998a60436effc14e878cdcc64d4676ff642396fda346e>

### Cost, measured on mainnet

Declaration fee was **17.55 STRK** (498,656,640 L2 gas) at an L2 gas price of
35,202,168,653 Fri.

That price also prices signature validation, which is the number that decides
whether this design is usable:

| Path | L2 gas | Cost at declare-time gas price |
|---|---|---|
| BIP-340 verification | 22,126,000 | **~0.78 STRK** |
| `is_valid_signature` end to end | 23,400,000 | **~0.82 STRK** |
| Range-check-only rejection | 18,900 | ~0.00067 STRK |

**Every transaction from a `NostrAccount` pays ~0.78 STRK before doing anything
useful** — roughly 1,171x the cheap reject path in the same contract, because
BIP-340 needs two secp256k1 scalar multiplications plus a tagged SHA-256 where a
Stark-curve account uses a native builtin.

Ten transactions is about 8 STRK in pure signature overhead. This is the accepted
cost of the design: the fork deliberately dropped the alternative (NIP-SW attested
wallet binding, which put an external wallet in charge at no per-transaction cost)
in favour of the Nostr key controlling the account directly. Budget accordingly —
the overhead is per transaction and does not amortise.

### Derivation confirmed by the sequencer (no deployment needed)

A `--dry-run` deploy of a throwaway account on mainnet confirmed the address
derivation, the transaction-hash construction, and on-chain BIP-340 verification
in one call, without funding or deploying anything.

```
throwaway pubkey  f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9
                  (BIP-340 published test vector 0 — a public value, deliberately
                   not secret, so no key was generated for this)
derived address   0x0010c66994979a83aa145f3c76a9593e6ee94dd63b97d4ba79cdc29eef8d8528
chain_id          0x534e5f4d41494e  (SN_MAIN)
l2_gas_consumed   26,146,934
overall_fee       923,108,179,789,507,228 Fri  (~0.9231 STRK)
```

Why estimation proves the derivation rather than merely pricing it:

1. `NostrAccountFactory::is_signer_interactive()` returns `false`, so
   `starknet-accounts` does **not** pass `SimulationFlagForEstimateFee::SkipValidate`
   (`factory/mod.rs:548-575`). `__validate_deploy__` really ran.
2. The `DEPLOY_ACCOUNT` v3 transaction hash includes the contract address
   (`hasher.update(self.address())`).
3. Our BIP-340 signature was made over the hash computed from *our* address.
4. The node independently recomputed the address and the hash, then ran
   `__validate_deploy__`, which verified that signature.
5. Validation passed. Had the node derived a different address, the hash would have
   differed and the signature could not have verified.

So the sequencer agrees with `buzz_core::starknet_account::account_address`. This is
the independent confirmation the unit tests could not give, since those and
`starknet-accounts` both route through the same `starknet-core` primitive.

It also proves BIP-340 verification works in the real VM on mainnet — not only in
`snforge` — and that the transaction-hash construction is right, since a wrong hash
would fail validation identically.

**Still unproven:** that a deploy transaction lands and the account exists
afterwards. Given the above, the marginal value of actually deploying is low: it
would cost ~0.92 STRK plus `sncast`-style padding, funded to an address controlled
by a publicly-known key.

### Deriving an account address

```bash
buzz wallet class-hash          # recompute after any contract change
buzz wallet address --pubkey <64-hex> \
  --class-hash 0x038a57ffba543e9fd54998a60436effc14e878cdcc64d4676ff642396fda346e
```

The address is a hash of the deployment parameters, so it exists before the
account does — fund it first, deploy later from its own balance.

**No account has been deployed yet**, but the derivation is confirmed — see
[above](#derivation-confirmed-by-the-sequencer-no-deployment-needed). The
sequencer validated a deploy against our derived address, so the formula and
inputs agree with the chain.

### Notes from the declare

- `tongo-deployer` holds ~9.95 STRK, short of even the unpadded 17.55 STRK
  estimate. `sncast` pads estimates into max bounds around 41 STRK, so keep well
  above the estimate.
- The RPC at `mainnet.nodes.starknet.org/rpc/v0_10` reports version
  `0.10.3-rc.0` where `sncast` 0.60 expects `0.10.0`. It warns and proceeds; the
  declare was unaffected.
- `starkli` is unmaintained and unused here. Tooling is `sncast` plus the
  `starknet-core` / `starknet-crypto` / `starknet-accounts` crates.
