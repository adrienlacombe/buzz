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
   `supplied_collateral`. **Rust rebuilds/validates the batch** before
   signing: fee transfer to `FEE_RECIPIENT` first, then approve /
   `execute_trade` against `DIFFICULTY_MARKET` + `COLLATERAL_TOKEN` only.
   Arbitrary frontend calls are rejected; `PlaceBetResult.feeAmount` alone
   is not the fee gate.
3. Rust signs SNIP-12 OutsideExecution with BIP-340 (`sign_tx_hash`) and
   submits via the AVNU proxy (`feeMode: sponsored`). Agent keyring slots
   (`agent:<pubkey>`) never receive a Starknet account.

Halt: wallet-owned (not indexer). Product signal is mempool.space
`GET /api/v1/difficulty-adjustment` — disable betting when
`remainingBlocks <= 24` (next retarget − 24). Tauri
`difficulty_halt_status` feeds the UI; `place_bet` re-fetches and refuses.
2016-block tip-height math remains as a unit-test / fallback helper only.
Operator settle/pause after retarget is out of scope here.

## INDEXER_URL

Required env (or the product public host). **No localhost default** — Adrien
does not want the indexer run locally. Loopback (`http://127.0.0.1:8787`) was
listing-proof only and must not ship.

```text
INDEXER_URL=https://markets.bitcoinmarkets.app
```

(`VITE_INDEXER_URL` is accepted in the desktop Vite bundle. Vite also exposes
`INDEXER_URL` via `envPrefix`. Packaged builds prefer the Tauri command
`markets_indexer_url`, which reads runtime `INDEXER_URL` so the documented
env var does not silently no-op.)

Listing/health (no auth):

- `GET {INDEXER_URL}/api/markets`
- `GET {INDEXER_URL}/health`

v1 market:

- `address`: `0x023b3a7bbe48a905ceadc17cd21b6b71fedaf90ee1218e462b106e01703b9cc8`
- `title`: Bitcoin difficulty after next retarget
- `marketType`: lognormal
- `xAxisLabel`: Difficulty
- collateral UI copy: **BTC**

Do **not** put indexer `ADMIN_API_KEY` or `AVNU_API_KEY` in the Buzz repo,
desktop client, or PR. Listing/health do not need `ADMIN_API_KEY`. Set
`AVNU_API_KEY` only on `buzz-avnu-proxy` at runtime.

Clients refuse loopback `INDEXER_URL` values. See `infra/aws/markets.tf.md`.

### prepareTrade (bet path)

Reuse `prepareTrade({ targetMean })` semantics with `targetMean = ln(D)`.
There is no SDK `prepareLognormalTrade`. Hints set both `l2_norm_denom` and
`backing_denom` to cairo `isqrt(2*sigma*sqrt_pi)` (same limbs). Calls:

`[strkBTC.transfer(feeRecipient, feeAmount), ...trade.calls]`

No `executeTrade()`. Do not bump approve / `supplied_collateral` for the fee.
**Do not mix Lightning into the bet path.**

## AVNU_API_KEY / AVNU_PROXY_URL

Product desktop talks to the public SNIP-29 proxy without a user-set env:

```text
AVNU_PROXY_URL=https://paymaster.bitcoinmarkets.app   # product host (default)
```

`AVNU_PROXY_URL` may override; unset / empty falls back to that host. Loopback
(`127.0.0.1`, `localhost`, `0.0.0.0`, `[::1]`) is refused. This is
`buzz-avnu-proxy`, **not** the old Nostr/STRK `buzz-paymaster` (`paymaster.tf`
stays off).

Set `AVNU_API_KEY` only on the hosted proxy (never in the Tauri binary, repo,
or client). Non-loopback `/rpc` requires Bearer
`AVNU_PROXY_AUTH_TOKEN` in the desktop process env at runtime — sourced from
AWS secret `buzz-dev/avnu-proxy` (never committed; never baked into the
client). Missing token fails closed; the header is never silently omitted.

Proxy process env (server-side only):

```text
AVNU_API_KEY=…          # from portal.avnu.fi — never commit
AVNU_PAYMASTER_URL=https://starknet.paymaster.avnu.fi
BIND_ADDR=0.0.0.0:8788  # non-loopback in AWS; requires PROXY_AUTH_TOKEN
PROXY_AUTH_TOKEN=…      # same secret material as AVNU_PROXY_AUTH_TOKEN
```

The proxy is **not** an unauthenticated open relay: there is no `CORS Any`,
and off-loopback requires Bearer. Health:
`GET https://paymaster.bitcoinmarkets.app/health` →
`{"service":"buzz-avnu-proxy","status":"ok"}`.

The API key never enters the client.
