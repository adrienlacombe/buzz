/**
 * Bet path: prepare lognormal trade + prepend feeCall, then hand Call[] to
 * Tauri `place_bet`. No Lightning / Atomiq / invoice on this path.
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

export function buildBetCalls(prepared: PreparedLognormalTrade): Call[] {
  const feeCall = buildFeeCall(prepared.tokenAmount);
  // fee first, then approve + execute_trade. Never mix LN here.
  return [feeCall, ...prepared.calls];
}

/**
 * Prepare and submit a curve bet. Signing stays in Rust via `place_bet`.
 */
export async function placeBet(params: PlaceBetParams): Promise<PlaceBetResult> {
  const prepared = prepareLognormalTrade({
    rawDifficulty: params.rawDifficulty,
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
