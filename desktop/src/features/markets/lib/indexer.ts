/**
 * Resolve the markets indexer base URL.
 *
 * `INDEXER_URL` (or `VITE_INDEXER_URL`) is **required**. There is no client
 * default — especially not `http://127.0.0.1:8787`. Localhost is listing-proof
 * only; Adrien does not want this run locally. Set a public host when ready
 * (expected: `https://markets.bitcoinmarkets.app`).
 *
 * Listing on whatever host the env points at:
 * - `GET {INDEXER_URL}/api/markets`
 * - `GET {INDEXER_URL}/health`
 */
export function resolveIndexerUrl(
  env: Record<string, string | undefined> = import.meta.env as Record<
    string,
    string | undefined
  >,
): string {
  const raw = env.VITE_INDEXER_URL?.trim() || env.INDEXER_URL?.trim() || "";
  if (!raw) {
    throw new Error(
      "INDEXER_URL is required (public host; no localhost default — do not use http://127.0.0.1:8787)",
    );
  }
  const base = raw.replace(/\/$/, "");
  if (/127\.0\.0\.1|localhost/i.test(base)) {
    throw new Error(
      "INDEXER_URL must not be loopback; localhost is listing-proof only. Set a public host (e.g. https://markets.bitcoinmarkets.app).",
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
