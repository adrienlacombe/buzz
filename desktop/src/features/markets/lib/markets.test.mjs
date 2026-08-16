import assert from "node:assert/strict";
import { describe, it } from "node:test";

const WALLET_FEE_BPS = 10n;
const RETARGET_INTERVAL = 2016;
const HALT_BLOCKS_BEFORE_RETARGET = 24;
const PRODUCT_INDEXER_URL = "https://markets.bitcoinmarkets.app";

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
    PRODUCT_INDEXER_URL;
  const base = raw.replace(/\/$/, "");
  if (/127\.0\.0\.1|localhost/i.test(base)) {
    throw new Error(
      "INDEXER_URL must not be loopback; use https://markets.bitcoinmarkets.app",
    );
  }
  return base;
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
  it("uses the product host, never loopback", () => {
    assert.equal(PRODUCT_INDEXER_URL, "https://markets.bitcoinmarkets.app");
    assert.equal(resolveIndexerUrl({}), PRODUCT_INDEXER_URL);
    assert.equal(
      resolveIndexerUrl({
        VITE_INDEXER_URL: "https://markets.bitcoinmarkets.app/",
      }),
      "https://markets.bitcoinmarkets.app",
    );
    assert.throws(
      () => resolveIndexerUrl({ INDEXER_URL: "http://127.0.0.1:8787" }),
      /must not be loopback/,
    );
  });
});
