/**
 * FORK-LOCAL FILE (adrienlacombe/buzz) — not present in block/buzz.
 *
 * The custom URL scheme the desktop app registers with the OS. This fork uses
 * `bitcoinmarkets` rather than upstream's `buzz`, so that installing it alongside
 * upstream Buzz does not leave the two contesting the same links with the OS
 * picking a winner non-deterministically.
 *
 * It lives in one place because the web client is what hands these URLs to the
 * OS: a link built with the wrong scheme opens the other app, or nothing at all,
 * and the failure is silent. Must stay in agreement with:
 *
 *   - `desktop/src-tauri/tauri.conf.json` → `plugins.deep-link.desktop.schemes`
 *   - `desktop/src-tauri/src/deep_link.rs` → `DEEP_LINK_SCHEME`
 *
 * Additive by design: a new file cannot conflict with an upstream edit, whereas
 * inlining the scheme at each call site would put fork-local strings into three
 * upstream components.
 */
export const DEEP_LINK_SCHEME = "bitcoinmarkets";

/** Build a deep link, e.g. `deepLink("join", params)`. */
export function deepLink(action: string, params: URLSearchParams): string {
  return `${DEEP_LINK_SCHEME}://${action}?${params.toString()}`;
}
