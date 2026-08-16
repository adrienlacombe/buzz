/**
 * Bitcoin Markets product constants.
 *
 * UI copy must never surface L2 vocabulary (Starknet, STRK, felts, strkBTC,
 * deploy, paymaster). Collateral is always labeled “BTC”.
 */

/**
 * Default indexer for listing the v1 market: Adrien's machine localhost
 * (same host, not sslip.io). Override with `VITE_INDEXER_URL` / `INDEXER_URL`.
 * Cloud agents cannot reach this host — do not live-fetch it in CI/agent.
 */
export const DEFAULT_INDEXER_URL = "http://127.0.0.1:8787";

/** @deprecated Use {@link DEFAULT_INDEXER_URL}. */
export const PRODUCT_INDEXER_URL = DEFAULT_INDEXER_URL;

/**
 * Live LOGNORMAL difficulty market.
 * Indexer may return the same address unpadded (`0x23b3…`); match via normalize.
 */
export const DIFFICULTY_MARKET =
  "0x023b3a7bbe48a905ceadc17cd21b6b71fedaf90ee1218e462b106e01703b9cc8";

/** Unpadded form as returned by `GET {INDEXER_URL}/api/markets`. */
export const DIFFICULTY_MARKET_UNPADDED =
  "0x23b3a7bbe48a905ceadc17cd21b6b71fedaf90ee1218e462b106e01703b9cc8";

export const MARKET_TITLE = "Bitcoin difficulty after next retarget";

export const MARKET_TYPE = "lognormal";

export const X_AXIS_LABEL = "Difficulty";

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
