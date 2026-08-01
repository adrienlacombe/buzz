/// Service name for the desktop OS keyring. Debug builds default to a distinct
/// service, while standalone worktree launches may request a scoped dev service.
fn dev_keyring_service(configured: Option<String>) -> String {
    configured
        .filter(|service| service.starts_with("buzz-desktop-dev."))
        .unwrap_or_else(|| "buzz-desktop-dev".to_string())
}

// FORK-LOCAL PATCH (adrienlacombe/buzz): the release service is
// "bitcoinmarkets-desktop", not "buzz-desktop".
//
// This constant does not key off the bundle identifier (see the note in
// secret_store.rs), so renaming the identifier alone would split the app-data
// directory and still leave this fork and upstream Buzz sharing one keychain
// entry — the identity and its store would then disagree, which is worse than
// sharing both. Both have to move together.
pub(crate) const RELEASE_KEYRING_SERVICE: &str = "bitcoinmarkets-desktop";

pub(crate) fn keyring_service() -> &'static str {
    if cfg!(debug_assertions) {
        static DEV_SERVICE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        DEV_SERVICE
            .get_or_init(|| dev_keyring_service(std::env::var("BUZZ_DEV_KEYRING_SERVICE").ok()))
            .as_str()
    } else {
        RELEASE_KEYRING_SERVICE
    }
}

pub(super) fn migration_marker_name(service: &str, default_name: &str) -> String {
    // FORK-LOCAL PATCH (adrienlacombe/buzz): RELEASE_KEYRING_SERVICE joins the
    // canonical list. Without it the fork's release build would fall through to
    // the scoped branch and namespace its marker, which is the behaviour meant
    // for per-worktree dev services — not for a release build.
    if service == RELEASE_KEYRING_SERVICE
        || service == "buzz-desktop"
        || service == "buzz-desktop-dev"
    {
        default_name.to_string()
    } else {
        format!("identity.{service}.migrated")
    }
}

#[cfg(test)]
mod tests {
    use super::{dev_keyring_service, migration_marker_name};

    #[test]
    fn standalone_scope_must_remain_under_dev_service() {
        assert_eq!(
            dev_keyring_service(Some("buzz-desktop-dev.example".to_string())),
            "buzz-desktop-dev.example"
        );
        assert_eq!(
            dev_keyring_service(Some("buzz-desktop".to_string())),
            "buzz-desktop-dev"
        );
    }

    #[test]
    fn standalone_scope_uses_its_own_migration_marker() {
        assert_eq!(
            migration_marker_name("buzz-desktop", "identity.migrated"),
            "identity.migrated"
        );
        assert_eq!(
            migration_marker_name("buzz-desktop-dev", "identity.migrated"),
            "identity.migrated"
        );
        assert_eq!(
            migration_marker_name("buzz-desktop-dev.example", "identity.migrated"),
            "identity.buzz-desktop-dev.example.migrated"
        );
    }
}
