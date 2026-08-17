/**
 * Resolve the markets AVNU SNIP-29 proxy base URL.
 *
 * Required deploy env `AVNU_PROXY_URL` / `VITE_AVNU_PROXY_URL`, or the product
 * public host `https://paymaster.bitcoinmarkets.app`. **No localhost default**
 * — loopback was local-only and must not ship.
 *
 * Never read or ship `AVNU_API_KEY` / proxy auth tokens here. UI copy must not
 * surface L2 vocabulary. Product host needs no Bearer; custom proxies still do
 * (handled in the Tauri `place_bet` path, not this module).
 */

import { PRODUCT_AVNU_PROXY_URL } from "./constants";

export function resolveAvnuProxyUrl(
  env: Record<string, string | undefined> = import.meta.env as Record<
    string,
    string | undefined
  >,
): string {
  const raw =
    env.VITE_AVNU_PROXY_URL?.trim() ||
    env.AVNU_PROXY_URL?.trim() ||
    PRODUCT_AVNU_PROXY_URL;
  const base = raw.replace(/\/$/, "");
  if (/127\.0\.0\.1|localhost|\[::1\]|0\.0\.0\.0/i.test(base)) {
    throw new Error(
      "AVNU_PROXY_URL must not be loopback; use https://paymaster.bitcoinmarkets.app (required env / public host, no localhost default)",
    );
  }
  return base;
}
