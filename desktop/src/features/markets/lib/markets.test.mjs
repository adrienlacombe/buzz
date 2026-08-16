import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { DIFFICULTY_MARKET, PRODUCT_INDEXER_URL } from "./constants.ts";
import { walletFeeAmount } from "./fee.ts";
import {
  bettingHalted,
  bettingHaltedByRemainingBlocks,
  haltHeight,
  nextRetargetHeight,
} from "./halt.ts";
import { findDifficultyMarket, resolveIndexerUrl } from "./indexer.ts";

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

  it("product signal: remainingBlocks <= 24", () => {
    assert.equal(bettingHaltedByRemainingBlocks(25), false);
    assert.equal(bettingHaltedByRemainingBlocks(24), true);
    assert.equal(bettingHaltedByRemainingBlocks(0), true);
  });
});

describe("INDEXER_URL", () => {
  it("uses product host; refuses localhost default", () => {
    assert.equal(PRODUCT_INDEXER_URL, "https://markets.bitcoinmarkets.app");
    assert.equal(resolveIndexerUrl({}), PRODUCT_INDEXER_URL);
    assert.equal(
      resolveIndexerUrl({
        INDEXER_URL: "https://markets.bitcoinmarkets.app/",
      }),
      "https://markets.bitcoinmarkets.app",
    );
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
    assert.throws(
      () => resolveIndexerUrl({ INDEXER_URL: "http://localhost:8787" }),
      /must not be loopback/,
    );
  });

  it("matches v1 listing row (padded address, BTC collateral copy)", () => {
    const listing = {
      address: DIFFICULTY_MARKET,
      title: "Bitcoin difficulty after next retarget",
      marketType: "lognormal",
      xAxisLabel: "Difficulty",
    };
    const found = findDifficultyMarket([listing], DIFFICULTY_MARKET);
    assert.equal(found?.address, DIFFICULTY_MARKET);
    assert.equal(found?.title, "Bitcoin difficulty after next retarget");
    assert.equal(found?.marketType, "lognormal");
    assert.equal(found?.xAxisLabel, "Difficulty");
  });

  it("fails closed when difficulty market is missing (no markets[0] substitute)", () => {
    const other = {
      address: "0xabc",
      title: "Some other market",
      marketType: "normal",
      xAxisLabel: "X",
    };
    assert.equal(findDifficultyMarket([other], DIFFICULTY_MARKET), null);
    assert.equal(findDifficultyMarket([], DIFFICULTY_MARKET), null);
  });
});
