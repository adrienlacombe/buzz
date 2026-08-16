/**
 * Bet path: reuse prepareTrade-style prep with targetMean = ln(D), then
 * prepend strkBTC.transfer(feeRecipient, feeAmount) and ...trade.calls.
 * No Lightning / Atomiq / invoice. No executeTrade(). No fee bump on
 * approve / supplied_collateral.
 */

import type { Call } from "starknet";

import { invokeTauri } from "@/shared/api/tauri";

import { buildFeeCall } from "./feeCall";
import {
  prepareLognormalTrade,
  type MarketSnapshot,
  type PreparedLognormalTrade,
} from "./prepareLognormalTrade";

export type PlaceBetParams = {
  rawDifficulty: number;
  /** User BTC amount to spend (required). */
  collateralBtc: number;
  market: MarketSnapshot;
  bitcoinHeight: number;
  targetVariance?: number;
  bufferPercent?: number;
};

export type PlaceBetResult = {
  txHash: string;
  summary: string;
  tokenAmount: string;
  feeAmount: string;
};

/**
 * `[feeTransfer, ...trade.calls]` — fee is a separate transfer only.
 */
export function buildBetCalls(prepared: PreparedLognormalTrade): Call[] {
  const feeCall = buildFeeCall(prepared.tokenAmount);
  return [feeCall, ...prepared.calls];
}

/**
 * Prepare and submit a curve bet. Signing stays in Rust via `place_bet`.
 */
export async function placeBet(
  params: PlaceBetParams,
): Promise<PlaceBetResult> {
  // prepareTrade({ targetMean: ln(D) }) equivalent for lognormal (same denoms).
  const prepared = prepareLognormalTrade({
    rawDifficulty: params.rawDifficulty,
    collateralBtc: params.collateralBtc,
    market: params.market,
    targetVariance: params.targetVariance,
    bufferPercent: params.bufferPercent,
  });
  const calls = buildBetCalls(prepared);

  const result = await invokeTauri<{
    txHash: string;
    feeAmount: string;
  }>("place_bet", {
    calls,
    bitcoinHeight: params.bitcoinHeight,
    tokenAmount: prepared.tokenAmount.toString(),
  });

  return {
    txHash: result.txHash,
    summary: prepared.summary,
    tokenAmount: prepared.tokenAmount.toString(),
    feeAmount: result.feeAmount,
  };
}
