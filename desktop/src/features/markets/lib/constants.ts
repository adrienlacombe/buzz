/**
 * Bitcoin Markets product constants.
 *
 * UI copy must never surface L2 vocabulary (Starknet, STRK, felts, strkBTC,
 * deploy, paymaster). Collateral is always labeled “BTC”.
 */

/**
 * Product indexer host. `INDEXER_URL` / `VITE_INDEXER_URL` may set this;
 * when unset the client uses this public host. **No localhost default** —
 * Adrien does not want the indexer run locally. Loopback (`127.0.0.1:8787`)
 * was listing-proof only and must not ship.
 *
 * Never put indexer `ADMIN_API_KEY` or `AVNU_API_KEY` in this repo/client.
 */
export const PRODUCT_INDEXER_URL = "https://markets.bitcoinmarkets.app";

/**
 * Product AVNU SNIP-29 proxy host. `AVNU_PROXY_URL` / `VITE_AVNU_PROXY_URL`
 * may set this; when unset the client uses this public host. **No localhost
 * default** — loopback (`127.0.0.1:8788`) must not ship.
 *
 * Never put `AVNU_API_KEY` or proxy auth tokens in this repo/client.
 */
export const PRODUCT_AVNU_PROXY_URL = "https://paymaster.bitcoinmarkets.app";

/** Live LOGNORMAL difficulty market (padded felt). */
export const DIFFICULTY_MARKET =
  "0x023b3a7bbe48a905ceadc17cd21b6b71fedaf90ee1218e462b106e01703b9cc8";

export const MARKET_TITLE = "Bitcoin difficulty after next retarget";

export const MARKET_TYPE = "lognormal";

export const X_AXIS_LABEL = "Difficulty";

/** Product label for collateral — always “BTC” in UI copy. */
export const COLLATERAL_LABEL = "BTC";

export const FACTORY =
  "0x046b18bbc9b0de137e4f919100ee6b61bf37d345f8099ff7f982b7eaffcab62d";

/** Collateral token (8 decimals). Product label: “BTC”. */
export const COLLATERAL_TOKEN =
  "0x0787150e306e6eae6e3f79dea881770e8bbff2c1b8eb490f969669ee945b3135";

export const FEE_RECIPIENT =
  "0x03df153485c79b693c42563d71abd315635a4819ba3415d07f8421b4ebc839c6";

export const NOSTR_ACCOUNT_CLASS_HASH =
  "0x0414f62ea1ed35f8c7bd3b794d94efc95e01bccf04e0f47211fc198f7f56f537";

export const WALLET_FEE_BPS = 10;

/** Protocol floor ~0.000977 BTC in 8-decimal raw units. */
export const MIN_TRADE_RAW = 97_700n;

export const RETARGET_INTERVAL = 2016;
export const HALT_BLOCKS_BEFORE_RETARGET = 24;

/** Lightning fund bounds (sats). */
export const LN_MIN_SATS = 100n;
export const LN_MAX_SATS = 2_000_000n;

export const COLLATERAL_DECIMALS = 8;
