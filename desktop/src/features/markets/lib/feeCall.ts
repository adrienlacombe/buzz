/**
 * Build the wallet fee transfer call (prepended before trade.calls).
 * Does NOT bump approve or supplied_collateral.
 */

import type { Call } from "starknet";

import { COLLATERAL_TOKEN, FEE_RECIPIENT } from "./constants";
import { u256Calldata, walletFeeAmount } from "./fee";

export function buildFeeCall(tokenAmount: bigint): Call {
  const fee = walletFeeAmount(tokenAmount);
  const [feeLow, feeHigh] = u256Calldata(fee);
  return {
    contractAddress: COLLATERAL_TOKEN,
    entrypoint: "transfer",
    calldata: [FEE_RECIPIENT, feeLow, feeHigh],
  };
}
