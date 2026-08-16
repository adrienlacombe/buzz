/**
 * Lognormal prepare path for the Bitcoin difficulty market.
 *
 * There is NO `prepareLognormalTrade` in `@the-situation/sdk` — only the
 * normal-family `prepareTrade({ targetMean })`. We reuse that contract:
 *   1. UI axis is raw difficulty `D`
 *   2. `targetMean = ln(D)`
 *   3. Hints: both `l2_norm_denom` and `backing_denom` = cairo
 *      `isqrt(2*sigma*sqrt_pi)` (identical limbs)
 *   4. Encode `execute_trade` (LOGNORMAL ABI uses `candidate.mu`)
 *   5. Return `calls = [approve(+5%), trade]` — caller prepends
 *      `strkBTC.transfer(feeRecipient, feeAmount)` then `...trade.calls`
 *
 * `supplied_collateral` is the **user's** BTC amount (must cover the solver
 * minimum). Do NOT call SDK `executeTrade()`. Do NOT bump approve /
 * `supplied_collateral` for the wallet fee.
 */

import { LOGNORMAL_AMM_ABI } from "@the-situation/artifacts";
import { LognormalDistribution, SQ128x128 } from "@the-situation/core";
import { findLognormalMinimum } from "@the-situation/collateral";
import { buildApproveCall, toHexAddress } from "@the-situation/utils";
import { CallData, type Call } from "starknet";

import {
  COLLATERAL_DECIMALS,
  COLLATERAL_TOKEN,
  DIFFICULTY_MARKET,
  MIN_TRADE_RAW,
} from "./constants";
import { computeLognormalHints } from "./lognormalHints";

export type MarketSnapshot = {
  /** Log-space μ (indexer `state.mean` for lognormal). */
  mu: number;
  variance: number;
  sigma: number;
  /** Effective k used for lambda scaling. */
  effectiveK: number;
};

export type PrepareLognormalTradeOptions = {
  /** Raw Bitcoin difficulty D from the UI axis (NOT ln). */
  rawDifficulty: number;
  /**
   * User-specified collateral in BTC (8dp human units). This is what gets
   * spent / supplied — not the solver's minimum alone.
   */
  collateralBtc: number;
  /** Optional target variance in log-space; defaults to current. */
  targetVariance?: number;
  /** Buffer percent applied when checking the solver floor (default 1%). */
  bufferPercent?: number;
  /** Current market snapshot (log-space). */
  market: MarketSnapshot;
  marketAddress?: string;
};

export type PreparedLognormalTrade = {
  /** ln(D) used as candidate μ. */
  targetMu: number;
  targetVariance: number;
  targetSigma: number;
  xStar: number;
  /** User collateral in human token units (what is supplied). */
  collateral: number;
  /** Solver minimum (scaled + buffered) the user must cover. */
  minimumCollateral: number;
  /** Raw 8dp token amount for approve / fee math (= user amount). */
  tokenAmount: bigint;
  /** [approveCall, tradeCall] — prepend feeCall before execute. */
  calls: Call[];
  summary: string;
};

function lognormalL2Norm(mu: number, variance: number): number {
  const sigma = Math.sqrt(Math.max(0, variance));
  if (sigma <= 0 || !Number.isFinite(sigma)) {
    return 0;
  }
  const denom = Math.sqrt(2 * sigma * Math.sqrt(Math.PI));
  const scale = Math.exp(variance / 8 - mu / 2);
  return scale / denom;
}

function lognormalLambda(mu: number, variance: number, k: number): number {
  const n = lognormalL2Norm(mu, variance);
  if (n <= 0 || !Number.isFinite(n)) {
    return 0;
  }
  return k / n;
}

function toTokenAmountUp(amount: number, decimals: number): bigint {
  const scale = 10 ** decimals;
  return BigInt(Math.ceil(amount * scale - Number.EPSILON));
}

function toAbiSq128(raw: {
  limb0: bigint;
  limb1: bigint;
  limb2: bigint;
  limb3: bigint;
  neg: boolean;
}) {
  return {
    limb0: raw.limb0,
    limb1: raw.limb1,
    limb2: raw.limb2,
    limb3: raw.limb3,
    neg: raw.neg,
  };
}

function requireSq(value: number, label: string): SQ128x128 {
  const sq = SQ128x128.fromNumber(value);
  if (!sq) {
    throw new Error(`Failed to encode ${label}`);
  }
  return sq;
}

/**
 * Prepare a lognormal curve bet. `rawDifficulty` is the UI axis value D;
 * internally the candidate mean is ln(D). Collateral spent is `collateralBtc`.
 */
export function prepareLognormalTrade(
  options: PrepareLognormalTradeOptions,
): PreparedLognormalTrade {
  const {
    rawDifficulty,
    collateralBtc,
    market,
    bufferPercent = 1,
    marketAddress = DIFFICULTY_MARKET,
  } = options;

  if (!(Number.isFinite(rawDifficulty) && rawDifficulty > 0)) {
    throw new Error("Target difficulty must be a positive number");
  }
  if (!(Number.isFinite(collateralBtc) && collateralBtc > 0)) {
    throw new Error("Collateral must be a positive BTC amount");
  }

  const targetMu = Math.log(rawDifficulty);
  const targetVariance = options.targetVariance ?? market.variance;
  if (!(Number.isFinite(targetVariance) && targetVariance > 0)) {
    throw new Error("Invalid target variance");
  }

  const currentMu = requireSq(market.mu, "market.mu");
  const currentVar = requireSq(market.variance, "market.variance");
  const candidateMu = requireSq(targetMu, "targetMu");
  const candidateVar = requireSq(targetVariance, "targetVariance");

  const current = LognormalDistribution.create(currentMu, currentVar);
  const candidate = LognormalDistribution.create(candidateMu, candidateVar);
  if (!(current && candidate)) {
    throw new Error("Failed to build lognormal distributions");
  }

  // Locate x* with the package Newton helper; user amount must cover the floor.
  const min = findLognormalMinimum(current, candidate);
  if (!min.converged || !Number.isFinite(min.collateral)) {
    throw new Error("Collateral solver failed for this target");
  }

  const lambdaF = lognormalLambda(
    market.mu,
    market.variance,
    market.effectiveK,
  );
  const lambdaG = lognormalLambda(targetMu, targetVariance, market.effectiveK);
  const scale = Math.max(lambdaF, lambdaG, market.effectiveK);
  const scaledMinimum = Math.max(0, min.collateral) * scale;
  const minimumCollateral = scaledMinimum * (1 + bufferPercent / 100);

  if (collateralBtc + Number.EPSILON < minimumCollateral) {
    throw new Error(
      `Collateral too low: need at least ${minimumCollateral.toFixed(6)} BTC for this target`,
    );
  }

  // Spend the user's amount — not only the solver minimum.
  const collateralSq = requireSq(collateralBtc, "collateralBtc");
  const xStarSq = requireSq(min.xStar, "xStar");

  const hints = computeLognormalHints(candidate.sigma);
  if (!hints) {
    throw new Error("Failed to compute lognormal hints");
  }

  const callData = new CallData(LOGNORMAL_AMM_ABI);
  const tradeCalldata = callData.compile("execute_trade", {
    candidate: {
      mu: toAbiSq128(candidate.toRaw().mu),
      variance: toAbiSq128(candidate.toRaw().variance),
      sigma: toAbiSq128(candidate.toRaw().sigma),
    },
    x_star: toAbiSq128(xStarSq.toRaw()),
    supplied_collateral: toAbiSq128(collateralSq.toRaw()),
    candidate_hints: {
      l2_norm_denom: toAbiSq128(hints.l2_norm_denom),
      backing_denom: toAbiSq128(hints.backing_denom),
    },
  });

  const tradeCall: Call = {
    contractAddress: toHexAddress(marketAddress),
    entrypoint: "execute_trade",
    calldata: tradeCalldata,
  };

  const tokenAmount = toTokenAmountUp(collateralBtc, COLLATERAL_DECIMALS);
  if (tokenAmount < MIN_TRADE_RAW) {
    throw new Error(
      `Minimum trade is ${(Number(MIN_TRADE_RAW) / 1e8).toFixed(6)} BTC`,
    );
  }

  // +5% on approve only — do not bump supplied_collateral for the wallet fee.
  const approveCall = buildApproveCall(
    COLLATERAL_TOKEN,
    marketAddress,
    tokenAmount,
  );

  return {
    targetMu,
    targetVariance,
    targetSigma: candidate.sigma.toNumber(),
    xStar: min.xStar,
    collateral: collateralBtc,
    minimumCollateral,
    tokenAmount,
    calls: [approveCall, tradeCall],
    summary: `Target difficulty ${rawDifficulty.toExponential(4)} (ln=${targetMu.toFixed(4)}), collateral ${collateralBtc.toFixed(6)} BTC`,
  };
}
