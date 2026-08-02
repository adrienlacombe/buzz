# Deployments

## `NostrAccount`

A Starknet account whose owner is a Nostr x-only pubkey, validating BIP-340
Schnorr signatures. See [`src/account.cairo`](src/account.cairo).

### Mainnet (`SN_MAIN`)

| | |
|---|---|
| Class hash | `0x0414f62ea1ed35f8c7bd3b794d94efc95e01bccf04e0f47211fc198f7f56f537` |
| Declare tx | `0x05328994e14ed537c34f3a19a79e4bad71d3be560fe47da4067dd7014c4399fc` |
| Status | `SUCCEEDED` / `ACCEPTED_ON_L2` |
| Declared with | `sncast --account snip36-e2e-bd1d4eaf` (0.60.0) |
| Cairo / Scarb | 2.18.0 |
| Declared | 2026-08-02 |

Verified after declaring, not assumed: `starknet_getClass` returns the class with
**10 external entry points and 1 constructor**, matching the local build exactly —
7 from the original account plus `execute_from_outside_v2`,
`is_valid_outside_execution_nonce` and `supports_interface` from SNIP-9. The
sequencer's class hash equals the one `buzz wallet class-hash` computes from
`target/dev/`.

Voyager: <https://voyager.online/class/0x0414f62ea1ed35f8c7bd3b794d94efc95e01bccf04e0f47211fc198f7f56f537>

#### The superseded class

`0x038a57ffba543e9fd54998a60436effc14e878cdcc64d4676ff642396fda346e` (declare tx
`0x04a5e5356cd64abd1fba1ce4a70e28e34ca993b3b25bb71d7b266be99c987d5a`) is the
pre-SNIP-9 class and is **still declared on mainnet** — a declare cannot be undone.

**Do not derive addresses from it.** It has no `execute_from_outside_v2`, so an
account at one of those addresses could never be sponsored, and since a fresh
account cannot pay its own ~0.78 STRK of BIP-340 verification it would be unable to
act at all.

Nothing was ever deployed against it, which is the only reason replacing it was
cheap. The address is a hash of the class, so every address moves with it; doing
this after the first deployment would have orphaned that account and anything sent
to it. That is why SNIP-9 landed before any deployment rather than after.

### Cost, measured on mainnet

Declaration fee was **22.7775 STRK actual** against a 22.8248 STRK estimate
(635,740,800 L2 gas at 35,902,687,417 Fri, plus 192 L1 data gas). `sncast` pads
estimates into max bounds around 51.4 STRK, so the account must *hold* far more
than the fee even though the surplus is returned.

#### What SNIP-9 added

|  | pre-SNIP-9 | with SNIP-9 | change |
|---|---|---|---|
| L2 gas | 498,656,640 | 635,740,800 | **+27.5%** |
| L2 gas price (Fri) | 35,202,168,653 | 35,902,687,417 | +2.0% |
| Declare fee (STRK) | 17.55 | 22.78 | +29.8% |

The gas price barely moved, so the increase is almost entirely **class size** — the
SRC5 and SRC9 components plus the nonce map. It is a one-time declare cost and does
**not** change the per-transaction cost, which is dominated by the ~22.1M gas of
BIP-340 verification and is untouched by sponsorship support.

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
  --class-hash 0x0414f62ea1ed35f8c7bd3b794d94efc95e01bccf04e0f47211fc198f7f56f537
```

The address is a hash of the deployment parameters, so it exists before the
account does — fund it first, deploy later from its own balance.

**No account has been deployed yet**, but the derivation is confirmed — see
[above](#derivation-confirmed-by-the-sequencer-no-deployment-needed). The
sequencer validated a deploy against our derived address, so the formula and
inputs agree with the chain.

### Notes from the declare

- **`sncast` pads estimates into max bounds far above the fee, and validation
  rejects on the bound, not the fee.** The 2026-08-02 declare cost 22.78 STRK but
  was rejected at a 32.30 STRK balance because the bound was 51.38 STRK — 2.3x the
  actual cost. Fund for the bound; the surplus is returned. `--dry-run --detailed`
  prints the real gas figures without sending anything, and `--l2-gas` /
  `--l2-gas-price` can tighten the bound if topping up is not an option.
- `tongo-deployer` holds ~9.95 STRK and cannot declare this contract.
  `snip36-e2e-bd1d4eaf` is the account both declares used.
- The RPC at `mainnet.nodes.starknet.org/rpc/v0_10` reports version
  `0.10.3-rc.0` where `sncast` 0.60 expects `0.10.0`. It warns and proceeds; both
  declares were unaffected. Note the hostname: `nodes.starknet.org`, not
  `starknet.nodes.org`.
- **After a declare, `starknet_getClass` and `getTransactionReceipt` briefly 404
  while `getTransactionStatus` already reports `ACCEPTED_ON_L2`/`SUCCEEDED`.** That
  is index lag on the node, not a failed declare — check status first and do not
  re-send.
- `starkli` is unmaintained and unused here. Tooling is `sncast` plus the
  `starknet-core` / `starknet-crypto` / `starknet-accounts` crates.
