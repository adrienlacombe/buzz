# Deployments

## `NostrAccount`

A Starknet account whose owner is a Nostr x-only pubkey, validating BIP-340
Schnorr signatures. See [`src/account.cairo`](src/account.cairo).

### Mainnet (`SN_MAIN`)

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

Ten transactions is about 8 STRK in pure signature overhead. Price this against
the alternative before building on it: NIP-SW
([`docs/nips/NIP-SW.md`](../docs/nips/NIP-SW.md)) gives attested wallet discovery
with an external wallet holding the funds, at no per-transaction cost.

### Deriving an account address

```bash
buzz wallet class-hash          # recompute after any contract change
buzz wallet address --pubkey <64-hex> \
  --class-hash 0x038a57ffba543e9fd54998a60436effc14e878cdcc64d4676ff642396fda346e
```

The address is a hash of the deployment parameters, so it exists before the
account does — fund it first, deploy later from its own balance.

**No account has been deployed yet.** Address derivation is therefore still
unverified against a real deployment: the formula and inputs are tested, but
nothing has confirmed the sequencer lands at the same address. Do not send funds
to a derived address until one deploy has proven it.

### Notes from the declare

- `tongo-deployer` holds ~9.95 STRK, short of even the unpadded 17.55 STRK
  estimate. `sncast` pads estimates into max bounds around 41 STRK, so keep well
  above the estimate.
- The RPC at `mainnet.nodes.starknet.org/rpc/v0_10` reports version
  `0.10.3-rc.0` where `sncast` 0.60 expects `0.10.0`. It warns and proceeds; the
  declare was unaffected.
- `starkli` is unmaintained and unused here. Tooling is `sncast` plus the
  `starknet-core` / `starknet-crypto` / `starknet-accounts` crates.
