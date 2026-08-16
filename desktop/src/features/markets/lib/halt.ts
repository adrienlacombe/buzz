import { HALT_BLOCKS_BEFORE_RETARGET, RETARGET_INTERVAL } from "./constants";

/** Next retarget height strictly after `currentHeight` (unit-test / fallback). */
export function nextRetargetHeight(currentHeight: number): number {
  const completed = Math.floor(currentHeight / RETARGET_INTERVAL);
  return (completed + 1) * RETARGET_INTERVAL;
}

/** Inclusive halt height: 24 blocks before the next retarget (fallback math). */
export function haltHeight(currentHeight: number): number {
  return nextRetargetHeight(currentHeight) - HALT_BLOCKS_BEFORE_RETARGET;
}

/**
 * Height-based betting halt helper (not wall-clock).
 * Product path prefers {@link bettingHaltedByRemainingBlocks} from mempool.
 */
export function bettingHalted(currentHeight: number): boolean {
  return currentHeight >= haltHeight(currentHeight);
}

/**
 * Product halt signal: mempool.space `remainingBlocks`.
 * Halt when `remainingBlocks <= 24`.
 */
export function bettingHaltedByRemainingBlocks(
  remainingBlocks: number,
): boolean {
  return remainingBlocks <= HALT_BLOCKS_BEFORE_RETARGET;
}

export type DifficultyHaltStatus = {
  remainingBlocks: number;
  nextRetargetHeight: number | null;
  halted: boolean;
  source: string;
};
