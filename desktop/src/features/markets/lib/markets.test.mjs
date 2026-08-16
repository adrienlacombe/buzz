import assert from "node:assert/strict";
import { describe, it } from "node:test";

const WALLET_FEE_BPS = 10n;
const RETARGET_INTERVAL = 2016;
const HALT_BLOCKS_BEFORE_RETARGET = 24;
const DEFAULT_INDEXER_URL = "http://127.0.0.1:8787";
const DIFFICULTY_MARKET =
  "0x023b3a7bbe48a905ceadc17cd21b6b71fedaf90ee1218e462b106e01703b9cc8";
const DIFFICULTY_MARKET_UNPADDED =
  "0x23b3a7bbe48a905ceadc17cd21b6b71fedaf90ee1218e462b106e01703b9cc8";

function walletFeeAmount(tokenAmount) {
  if (tokenAmount <= 0n) return 0n;
  const fee = (tokenAmount * WALLET_FEE_BPS + 9_999n) / 10_000n;
  return fee < 1n ? 1n : fee;
}

function nextRetargetHeight(currentHeight) {
  return (Math.floor(currentHeight / RETARGET_INTERVAL) + 1) * RETARGET_INTERVAL;
}

function haltHeight(currentHeight) {
  return nextRetargetHeight(currentHeight) - HALT_BLOCKS_BEFORE_RETARGET;
}

function bettingHalted(currentHeight) {
  return currentHeight >= haltHeight(currentHeight);
}

function resolveIndexerUrl(env = {}) {
  const raw =
    (env.VITE_INDEXER_URL || "").trim() ||
    (env.INDEXER_URL || "").trim() ||
    DEFAULT_INDEXER_URL;
  return raw.replace(/\/$/, "");
}

function normalizeMarketAddress(address) {
  const hex = address.trim().toLowerCase().replace(/^0x/, "");
  const stripped = hex.replace(/^0+/, "") || "0";
  return `0x${stripped}`;
}

function findDifficultyMarket(markets, difficultyMarketAddress) {
  const want = normalizeMarketAddress(difficultyMarketAddress);
  return (
    markets.find((m) => normalizeMarketAddress(m.address) === want) ??
    markets[0] ??
    null
  );
}

describe("walletFeeAmount", () => {
  it("charges 10 bps with ceil", () => {
    assert.equal(walletFeeAmount(10_000n), 10n);
    assert.equal(walletFeeAmount(1_000n), 1n);
    assert.equal(walletFeeAmount(2_000_000n), 2_000n);
  });

  it("floors at 1 sat when the product would be 0", () => {
    assert.equal(walletFeeAmount(1n), 1n);
    assert.equal(walletFeeAmount(50n), 1n);
    assert.equal(walletFeeAmount(999n), 1n);
  });

  it("returns 0 for zero amount", () => {
    assert.equal(walletFeeAmount(0n), 0n);
  });
});

describe("betting halt at height", () => {
  it("retargets every 2016 blocks", () => {
    assert.equal(nextRetargetHeight(0), 2016);
    assert.equal(nextRetargetHeight(2015), 2016);
    assert.equal(nextRetargetHeight(2016), 4032);
  });

  it("halts 24 blocks before the next retarget", () => {
    assert.equal(haltHeight(1000), 1992);
    assert.equal(bettingHalted(1991), false);
    assert.equal(bettingHalted(1992), true);
    assert.equal(bettingHalted(2015), true);
    assert.equal(bettingHalted(2016), false);
    assert.equal(haltHeight(2016), 4008);
    assert.equal(bettingHalted(4008), true);
  });
});

describe("INDEXER_URL", () => {
  it("defaults to Adrien localhost indexer (configurable)", () => {
    assert.equal(DEFAULT_INDEXER_URL, "http://127.0.0.1:8787");
    assert.equal(resolveIndexerUrl({}), DEFAULT_INDEXER_URL);
    assert.equal(
      resolveIndexerUrl({ INDEXER_URL: "http://127.0.0.1:8787/" }),
      "http://127.0.0.1:8787",
    );
    assert.equal(
      resolveIndexerUrl({
        VITE_INDEXER_URL: "https://markets.bitcoinmarkets.app/",
      }),
      "https://markets.bitcoinmarkets.app",
    );
  });

  it("matches unpadded indexer listing address to padded constant", () => {
    const listing = {
      address: DIFFICULTY_MARKET_UNPADDED,
      title: "Bitcoin difficulty after next retarget",
      marketType: "lognormal",
      xAxisLabel: "Difficulty",
    };
    assert.equal(
      normalizeMarketAddress(DIFFICULTY_MARKET),
      normalizeMarketAddress(DIFFICULTY_MARKET_UNPADDED),
    );
    const found = findDifficultyMarket([listing], DIFFICULTY_MARKET);
    assert.equal(found?.address, DIFFICULTY_MARKET_UNPADDED);
    assert.equal(found?.marketType, "lognormal");
    assert.equal(found?.xAxisLabel, "Difficulty");
  });
});
