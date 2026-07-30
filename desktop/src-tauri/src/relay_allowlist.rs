//! FORK-LOCAL (adrienlacombe/buzz) — not present in `block/buzz`.
//!
//! Restricts which relay hosts the desktop app may open a WebSocket to.
//!
//! Upstream Buzz is deliberately multi-community: a user can add any relay from
//! the community switcher, a `buzz://` deep link, or `BUZZ_RELAY_URL`. This fork
//! runs a single relay and wants the shipped client to reach only that one, so
//! every entry point is funnelled through [`ensure_relay_allowed`].
//!
//! **This is a configuration lock, not a security boundary.** It stops the
//! shipped app from talking to another relay. It cannot stop someone who
//! rebuilds the client, runs `buzz-cli`, or points any other Nostr client at a
//! relay — enforce *who may use our relay* server-side instead
//! (`BUZZ_REQUIRE_RELAY_MEMBERSHIP`, relay membership).
//!
//! The guard lives at the transport layer (`native_websocket::open_connection`)
//! rather than in the UI, because that is the one place every session must pass
//! through: community add, stored communities, deep links, the read-only
//! observer client, and reconnects all end up there.

/// Hosts this build may connect to, as a comma-separated list.
///
/// Overridable at build time so the fork's relay hostname is not baked into
/// logic that outlives it:
///
/// ```sh
/// BUZZ_DESKTOP_BUILD_RELAY_ALLOWLIST=relay.example.com pnpm tauri build
/// ```
///
/// Setting it to an empty string disables the restriction entirely, which is the
/// escape hatch for building an unrestricted client from this fork.
const DEFAULT_ALLOWLIST: &str = "relay.bitcoinmarkets.app";

fn configured_allowlist() -> &'static str {
    match option_env!("BUZZ_DESKTOP_BUILD_RELAY_ALLOWLIST") {
        Some(value) => value,
        None => DEFAULT_ALLOWLIST,
    }
}

/// The relay URL a build with no explicit configuration should use.
///
/// `None` in a debug build, and whenever the allowlist is disabled, so local
/// development keeps its `ws://localhost:3000` default.
///
/// In a release build this returns `wss://<first allowed host>`. Without it the
/// shipped app would default to loopback — which [`ensure_relay_allowed`] then
/// rejects — leaving a client that cannot connect to anything at all.
pub fn default_relay_url() -> Option<String> {
    if cfg!(debug_assertions) {
        return None;
    }
    configured_allowlist()
        .split(',')
        .map(str::trim)
        .find(|entry| !entry.is_empty())
        .map(|host| format!("wss://{host}"))
}

/// True when the host is a loopback address.
///
/// Local development (`just dev`, `just desktop-dev`) and every Playwright E2E
/// spec talk to `ws://localhost:3000`. Blocking that in a debug build would
/// break the entire local workflow, so loopback is allowed there — and only
/// there. A release build rejects it like any other off-list host.
fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

/// Extract the host from a ws/wss/http/https URL, lowercased, without the port.
///
/// Hand-rolled rather than pulling in a URL crate: this runs on a hot-ish path
/// and only needs the authority component.
fn host_of(url: &str) -> Option<String> {
    let rest = url
        .split_once("://")
        .map(|(_scheme, rest)| rest)
        .unwrap_or(url);
    // Strip path/query/fragment, then userinfo, then port.
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|value| !value.is_empty())?;
    let authority = authority
        .rsplit_once('@')
        .map(|(_userinfo, host)| host)
        .unwrap_or(authority);

    // IPv6 literals keep their brackets; a port follows the closing bracket.
    let host = if let Some(end) = authority.find(']') {
        &authority[..=end]
    } else {
        authority.split(':').next()?
    };

    let host = host.trim().to_ascii_lowercase();
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

/// Returns `Ok(())` when `url` may be connected to, otherwise a message safe to
/// surface to the UI.
pub fn ensure_relay_allowed(url: &str) -> Result<(), String> {
    let allowlist = configured_allowlist();
    if allowlist.trim().is_empty() {
        return Ok(());
    }

    let Some(host) = host_of(url) else {
        return Err(format!("Refusing to connect: no host in relay URL {url:?}"));
    };

    if cfg!(debug_assertions) && is_loopback(&host) {
        return Ok(());
    }

    let allowed = allowlist
        .split(',')
        .map(|entry| entry.trim().to_ascii_lowercase())
        .filter(|entry| !entry.is_empty())
        .any(|entry| entry == host);

    if allowed {
        Ok(())
    } else {
        Err(format!(
            "This build only connects to {allowlist}. Refusing relay host {host:?}."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{ensure_relay_allowed, host_of};

    #[test]
    fn extracts_host_from_various_urls() {
        assert_eq!(
            host_of("wss://relay.example.com").as_deref(),
            Some("relay.example.com")
        );
        assert_eq!(
            host_of("wss://relay.example.com/").as_deref(),
            Some("relay.example.com")
        );
        assert_eq!(
            host_of("ws://relay.example.com:3000/x?y#z").as_deref(),
            Some("relay.example.com")
        );
        assert_eq!(
            host_of("https://RELAY.Example.COM").as_deref(),
            Some("relay.example.com")
        );
        assert_eq!(
            host_of("wss://user:pw@relay.example.com").as_deref(),
            Some("relay.example.com")
        );
        assert_eq!(
            host_of("relay.example.com:3000").as_deref(),
            Some("relay.example.com")
        );
        assert_eq!(host_of("ws://[::1]:3000").as_deref(), Some("[::1]"));
        assert_eq!(host_of("wss://"), None);
        assert_eq!(host_of(""), None);
    }

    // Derived from the configured allowlist rather than hardcoded, so overriding
    // BUZZ_DESKTOP_BUILD_RELAY_ALLOWLIST at build time does not fail the suite.
    fn first_allowed_host() -> String {
        super::configured_allowlist()
            .split(',')
            .map(str::trim)
            .find(|entry| !entry.is_empty())
            .expect("test build must configure at least one allowed host")
            .to_string()
    }

    #[test]
    fn allows_the_configured_host() {
        let host = first_allowed_host();
        assert!(ensure_relay_allowed(&format!("wss://{host}")).is_ok());
        assert!(ensure_relay_allowed(&format!("wss://{host}/")).is_ok());
        assert!(ensure_relay_allowed(&format!("wss://{host}:443/x?y")).is_ok());
        // Host comparison is case-insensitive.
        assert!(ensure_relay_allowed(&format!("WSS://{}", host.to_uppercase())).is_ok());
    }

    #[test]
    fn rejects_other_hosts() {
        let host = first_allowed_host();
        let cases = vec![
            "wss://relay.damus.io".to_string(),
            // Suffix attack: allowed host as a prefix of an attacker domain.
            format!("wss://{host}.evil.example"),
            // Allowed host appearing only in the path, not the authority.
            format!("wss://evil.example/{host}"),
            // Allowed host in userinfo rather than the authority.
            format!("wss://{host}@relay.damus.io"),
        ];
        for url in cases {
            assert!(
                ensure_relay_allowed(&url).is_err(),
                "expected {url} to be rejected"
            );
        }
    }

    #[test]
    fn rejects_urls_without_a_host() {
        assert!(ensure_relay_allowed("wss://").is_err());
        assert!(ensure_relay_allowed("").is_err());
    }

    // A release build must not fall back to loopback, or the shipped client
    // would default to a host its own allowlist rejects.
    #[test]
    fn default_relay_url_follows_build_profile() {
        let default = super::default_relay_url();
        if cfg!(debug_assertions) {
            assert_eq!(default, None, "debug builds keep the loopback default");
        } else {
            let url = default.expect("release builds must supply an allowed default");
            assert!(url.starts_with("wss://"), "got {url}");
            assert!(
                ensure_relay_allowed(&url).is_ok(),
                "the default must satisfy the allowlist, got {url}"
            );
        }
    }

    // Loopback is permitted only in debug builds, so this test asserts the
    // behaviour of whichever profile it is compiled into.
    #[test]
    fn loopback_follows_build_profile() {
        let result = ensure_relay_allowed("ws://localhost:3000");
        if cfg!(debug_assertions) {
            assert!(result.is_ok(), "debug builds must allow local dev relays");
        } else {
            assert!(result.is_err(), "release builds must reject loopback");
        }
    }
}
