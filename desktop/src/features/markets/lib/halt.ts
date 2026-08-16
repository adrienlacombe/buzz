import { HALT_BLOCKS_BEFORE_RETARGET, RETARGET_INTERVAL } from "./constants";

/** Next retarget height strictly after `currentHeight`. */
export function nextRetargetHeight(currentHeight: number): number {
  const completed = Math.floor(currentHeight / RETARGET_INTERVAL);
  return (completed + 1) * RETARGET_INTERVAL;
}

/** Inclusive halt height: 24 blocks before the next retarget. */
export function haltHeight(currentHeight: number): number {
  return nextRetargetHeight(currentHeight) - HALT_BLOCKS_BEFORE_RETARGET;
}

/** Height-based betting halt (not wall-clock). */
export function bettingHalted(currentHeight: number): boolean {
  return currentHeight >= haltHeight(currentHeight);
}
