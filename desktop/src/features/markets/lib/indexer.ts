/**
 * Resolve the markets indexer base URL.
 *
 * Required deploy env `INDEXER_URL` / `VITE_INDEXER_URL`, or the product
 * public host `https://markets.bitcoinmarkets.app`. **No localhost default**
 * — Adrien does not want this run locally. Loopback was listing-proof only.
 *
 * Listing/health (no auth):
 * - `GET {INDEXER_URL}/api/markets`
 * - `GET {INDEXER_URL}/health`
 *
 * Never read or ship indexer `ADMIN_API_KEY` / `AVNU_API_KEY` here.
 */

import { PRODUCT_INDEXER_URL } from "./constants";

export function resolveIndexerUrl(
  env: Record<string, string | undefined> = import.meta.env as Record<
    string,
    string | undefined
  >,
): string {
  const raw =
    env.VITE_INDEXER_URL?.trim() ||
    env.INDEXER_URL?.trim() ||
    PRODUCT_INDEXER_URL;
  const base = raw.replace(/\/$/, "");
  if (/127\.0\.0\.1|localhost/i.test(base)) {
    throw new Error(
      "INDEXER_URL must not be loopback; use https://markets.bitcoinmarkets.app (required env / public host, no localhost default)",
    );
  }
  return base;
}

/** Normalize a felt hex for equality (strip 0x, leading zeros; lowercase). */
export function normalizeMarketAddress(address: string): string {
  const hex = address.trim().toLowerCase().replace(/^0x/, "");
  const stripped = hex.replace(/^0+/, "") || "0";
  return `0x${stripped}`;
}

export type IndexerMarket = {
  address: string;
  title: string;
  marketType?: string;
  /** Axis label from indexer (v1: "Difficulty"). */
  xAxisLabel?: string;
  state?: {
    mean: number | null;
    sigma: number | null;
    variance: number | null;
    k: number | null;
    effectiveK: number | null;
    isInitialized: boolean;
    isPaused: boolean;
    isSettled: boolean;
    totalBacking: number | null;
  } | null;
};

/** Unauthenticated listing fetch — never sends ADMIN_API_KEY. */
export async function fetchIndexerHealth(
  baseUrl = resolveIndexerUrl(),
): Promise<{ status: string }> {
  const res = await fetch(`${baseUrl}/health`);
  if (!res.ok) {
    throw new Error(`Indexer health failed: HTTP ${res.status}`);
  }
  return (await res.json()) as { status: string };
}

/** Unauthenticated listing fetch — never sends ADMIN_API_KEY. */
export async function fetchIndexerMarkets(
  baseUrl = resolveIndexerUrl(),
): Promise<IndexerMarket[]> {
  const res = await fetch(`${baseUrl}/api/markets`);
  if (!res.ok) {
    throw new Error(`Indexer markets failed: HTTP ${res.status}`);
  }
  return (await res.json()) as IndexerMarket[];
}

/**
 * Fail closed: only the difficulty market address matches.
 * Never substitute `markets[0]` under the difficulty title.
 */
export function findDifficultyMarket(
  markets: IndexerMarket[],
  difficultyMarketAddress: string,
): IndexerMarket | null {
  const want = normalizeMarketAddress(difficultyMarketAddress);
  return (
    markets.find((m) => normalizeMarketAddress(m.address) === want) ?? null
  );
}
