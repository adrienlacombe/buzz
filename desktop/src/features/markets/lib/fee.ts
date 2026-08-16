import { WALLET_FEE_BPS } from "./constants";

/**
 * Wallet fee: ceil(tokenAmount * 10 / 10_000), minimum 1 sat when amount > 0.
 * Do not bump approve or supplied_collateral by this amount.
 */
export function walletFeeAmount(tokenAmount: bigint): bigint {
  if (tokenAmount <= 0n) {
    return 0n;
  }
  const fee = (tokenAmount * BigInt(WALLET_FEE_BPS) + 9_999n) / 10_000n;
  return fee < 1n ? 1n : fee;
}

/** Split a u128 amount into Starknet u256 low/high hex limbs. */
export function u256Calldata(amount: bigint): [string, string] {
  const mask = (1n << 128n) - 1n;
  const low = amount & mask;
  const high = amount >> 128n;
  return [`0x${low.toString(16)}`, `0x${high.toString(16)}`];
}
