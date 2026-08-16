/**
 * Resolve the markets indexer base URL for listing the v1 market.
 *
 * Configurable via `VITE_INDEXER_URL` / `INDEXER_URL`. Default is Adrien's
 * local indexer on the same host as his machine (not sslip.io):
 * `http://127.0.0.1:8787`.
 *
 * Listing/health on whatever host INDEXER_URL points at:
 * - `GET {INDEXER_URL}/api/markets`
 * - `GET {INDEXER_URL}/health`
 *
 * Cloud / CI agents cannot reach Adrien's localhost — do not treat a live
 * fetch of the default URL as a build dependency.
 */

import { DEFAULT_INDEXER_URL } from "./constants";

export function resolveIndexerUrl(
  env: Record<string, string | undefined> = import.meta.env as Record<
    string,
    string | undefined
  >,
): string {
  const raw =
    env.VITE_INDEXER_URL?.trim() ||
    env.INDEXER_URL?.trim() ||
    DEFAULT_INDEXER_URL;
  return raw.replace(/\/$/, "");
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

export async function fetchIndexerHealth(
  baseUrl = resolveIndexerUrl(),
): Promise<{ status: string }> {
  const res = await fetch(`${baseUrl}/health`);
  if (!res.ok) {
    throw new Error(`Indexer health failed: HTTP ${res.status}`);
  }
  return (await res.json()) as { status: string };
}

export async function fetchIndexerMarkets(
  baseUrl = resolveIndexerUrl(),
): Promise<IndexerMarket[]> {
  const res = await fetch(`${baseUrl}/api/markets`);
  if (!res.ok) {
    throw new Error(`Indexer markets failed: HTTP ${res.status}`);
  }
  return (await res.json()) as IndexerMarket[];
}

export function findDifficultyMarket(
  markets: IndexerMarket[],
  difficultyMarketAddress: string,
): IndexerMarket | null {
  const want = normalizeMarketAddress(difficultyMarketAddress);
  return (
    markets.find((m) => normalizeMarketAddress(m.address) === want) ??
    markets[0] ??
    null
  );
}
