/**
 * Resolve the markets indexer base URL for listing the v1 market.
 *
 * Configurable via `VITE_INDEXER_URL` / `INDEXER_URL` so the host can be
 * swapped later. Default (no public hostname yet): Adrien's shared-machine
 * localhost `http://127.0.0.1:8787`.
 *
 * Public listing endpoints only (no auth):
 * - `GET {INDEXER_URL}/api/markets`
 * - `GET {INDEXER_URL}/health`
 *
 * `ADMIN_API_KEY` lives only on the indexer host — never read it, and never
 * put it in the Buzz repo, desktop client, or PR. Listing/health do not need it.
 *
 * Cloud VMs cannot reach Adrien's localhost; wire the desktop client only and
 * do not live-fetch this default from CI/agent environments.
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
