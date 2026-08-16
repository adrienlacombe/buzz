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

Required for deploy. Production:

```text
INDEXER_URL=https://markets.bitcoinmarkets.app
```

- `GET {INDEXER_URL}/api/markets`
- `GET {INDEXER_URL}/health`

Never ship a loopback default (`http://127.0.0.1:8787` is listing-proof only).
The desktop client uses the product host above (overridable via
`VITE_INDEXER_URL` / `INDEXER_URL`) and refuses loopback.

## AVNU_API_KEY

Set only on `buzz-avnu-proxy`:

```text
AVNU_API_KEY=…          # from portal.avnu.fi — never commit
AVNU_PAYMASTER_URL=https://starknet.paymaster.avnu.fi
BIND_ADDR=0.0.0.0:8788
```

Run: `cargo run -p buzz-avnu-proxy`. Desktop uses `AVNU_PROXY_URL` to reach
the proxy; the key never enters the Tauri binary.
