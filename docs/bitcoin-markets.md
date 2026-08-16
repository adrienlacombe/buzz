# Bitcoin Markets (hidden stack)

Product UX is Bitcoin-only. Users fund with Lightning and place a curve bet on
**Bitcoin difficulty after next retarget**. They never see Starknet, STRK,
felts, class hashes, strkBTC, deploy, or paymaster.

## Identity

One secp256k1 Nostr `nsec` in the OS keyring (`identity`). Agent keys in the
same keyring blob (`agent:<pubkey>`) **do not** get Starknet accounts.

## Fund (Lightning only)

`fund_lightning` (Tauri) returns the human counterfactual address. The Fund
screen uses Atomiq `@atomiqlabs/sdk` `FROM_BTCLN_AUTO` into that address
(token product-labeled **BTC**, min 100 / max 2_000_000 sats). Optional
`gasAmount` STRK drop is fallback only — AVNU sponsors gas for bets.

## Bet (Starknet only — hidden)

`place_bet` never creates an LN invoice, zap, or Atomiq swap. Flow:

1. JS `prepareLognormalTrade`: UI axis is raw difficulty `D`; candidate μ is
   `ln(D)`. Hints set **both** `l2_norm_denom` and `backing_denom` to
   `isqrt(2·σ·√π)` (same limbs). Normal `computeHints` must not be used.
2. Calls = `[feeCall, approve(+5%), execute_trade]`. Wallet fee is 10 bps
   (min 1 sat) as a separate transfer — do not bump approve or
   `supplied_collateral`.
3. Rust signs SNIP-12 OutsideExecution with BIP-340 (`sign_tx_hash`) and
   submits via the AVNU proxy (`feeMode: sponsored`).

Halt: wallet disables betting 24 Bitcoin blocks before the next retarget
height (every 2016 blocks).

## INDEXER_URL

CEO-confirmed listing base for the desktop client. Configurable so the host
can be swapped later; **no public hostname yet**. Default:

```text
INDEXER_URL=http://127.0.0.1:8787
```

(Adrien's shared machine localhost. Override with `INDEXER_URL` /
`VITE_INDEXER_URL`.)

Unauthenticated listing/health only:

- `GET {INDEXER_URL}/api/markets`
- `GET {INDEXER_URL}/health`

v1 market row:

- `address`: `0x023b3a7bbe48a905ceadc17cd21b6b71fedaf90ee1218e462b106e01703b9cc8`
- `title`: Bitcoin difficulty after next retarget
- `marketType`: lognormal
- `xAxisLabel`: Difficulty
- collateral UI copy: **BTC**

`ADMIN_API_KEY` exists **only on the indexer host**. Do **not** read it, and do
**not** put it in the Buzz repo, desktop client, or PR. Listing/health do not
need it.

Cloud VMs cannot reach Adrien's localhost — wire the **desktop client only**;
do not live-fetch the default URL from CI/agent. See `infra/aws/markets.tf.md`.

### prepareTrade (bet path)

Reuse `prepareTrade({ targetMean })` semantics with `targetMean = ln(D)`.
There is no SDK `prepareLognormalTrade`. Hints set both `l2_norm_denom` and
`backing_denom` to cairo `isqrt(2*sigma*sqrt_pi)` (same limbs). Calls:

`[strkBTC.transfer(feeRecipient, feeAmount), ...trade.calls]`

No `executeTrade()`. Do not bump approve / `supplied_collateral` for the fee.

## AVNU_API_KEY

Set only on `buzz-avnu-proxy`:

```text
AVNU_API_KEY=…          # from portal.avnu.fi — never commit
AVNU_PAYMASTER_URL=https://starknet.paymaster.avnu.fi
BIND_ADDR=0.0.0.0:8788
```

Run: `cargo run -p buzz-avnu-proxy`. Desktop uses `AVNU_PROXY_URL` to reach
the proxy; the key never enters the Tauri binary.
