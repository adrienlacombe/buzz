import { computeL2NormDenomHint } from "@the-situation/utils";
import { SQ128x128 } from "@the-situation/core";

export type Sq128Raw = {
  limb0: bigint;
  limb1: bigint;
  limb2: bigint;
  limb3: bigint;
  neg: boolean;
};

export type LognormalSqrtHints = {
  l2_norm_denom: Sq128Raw;
  backing_denom: Sq128Raw;
};

/**
 * Both denoms = isqrt(2 * sigma * sqrt_pi), identical limbs.
 * Do NOT use normal computeHints (two different denoms → revert).
 *
 * `SQ128x128` is the class type itself (private constructor makes
 * `InstanceType<typeof SQ128x128>` invalid under TS).
 */
export function computeLognormalHints(
  sigma: SQ128x128,
): LognormalSqrtHints | null {
  const denom = computeL2NormDenomHint(sigma);
  if (!denom) {
    return null;
  }
  const raw = denom.toRaw();
  return {
    l2_norm_denom: raw,
    backing_denom: { ...raw },
  };
}

export function computeLognormalHintsFromNumber(
  sigma: number,
): LognormalSqrtHints {
  const sigmaSq = SQ128x128.fromNumber(sigma);
  if (!sigmaSq) {
    throw new Error(`Failed to convert sigma=${sigma} to SQ128x128`);
  }
  const hints = computeLognormalHints(sigmaSq);
  if (!hints) {
    throw new Error(`Failed to compute lognormal hints for sigma=${sigma}`);
  }
  return hints;
}

/** Test helper: denoms must be limb-identical. */
export function hintsDenomsMatch(hints: LognormalSqrtHints): boolean {
  const a = hints.l2_norm_denom;
  const b = hints.backing_denom;
  return (
    a.limb0 === b.limb0 &&
    a.limb1 === b.limb1 &&
    a.limb2 === b.limb2 &&
    a.limb3 === b.limb3 &&
    a.neg === b.neg
  );
}
