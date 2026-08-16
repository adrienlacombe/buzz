import { PRODUCT_INDEXER_URL } from "./constants";

/**
 * Resolve the markets indexer base URL.
 *
 * Production host is `https://markets.bitcoinmarkets.app`. Loopback
 * (`127.0.0.1` / `localhost`) is listing-proof only and must never ship as the
 * client default.
 */
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
      "INDEXER_URL must not be loopback; use https://markets.bitcoinmarkets.app",
    );
  }
  return base;
}

export type IndexerMarket = {
  address: string;
  title: string;
  marketType?: string;
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
