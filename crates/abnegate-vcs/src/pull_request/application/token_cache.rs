use crate::pull_request::application::InstallationId;
use crate::pull_request::application::InstallationToken;
use std::collections::HashMap;
use std::sync::PoisonError;
use std::sync::RwLock;
use std::time::SystemTime;

/// An installation token per installation, held until it comes within
/// [`InstallationToken::SAFETY_MARGIN`] of expiring.
///
/// Every method returns owned data, so no lock is ever held across an
/// `await`, and a lock a panicking thread left poisoned is used as it stands,
/// since no method leaves the map part way through a change.
#[derive(Debug, Default)]
pub struct TokenCache {
    entries: RwLock<HashMap<InstallationId, InstallationToken>>,
}

impl TokenCache {
    /// A cache holding no tokens.
    pub fn new() -> Self {
        Self::default()
    }

    /// The token held for `installation`, if there is one and it is still
    /// usable at `now`.
    pub fn get(&self, installation: InstallationId, now: SystemTime) -> Option<InstallationToken> {
        self.entries
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&installation)
            .filter(|token| token.usable_at(now))
            .cloned()
    }

    /// Hold `token` for `installation`, in place of any token held for it.
    pub fn insert(&self, installation: InstallationId, token: InstallationToken) {
        self.entries
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(installation, token);
    }

    /// Drop the token held for `installation`, so the next request mints a
    /// new one: for when GitHub refuses it, as it does every token issued
    /// before an installation's permissions changed.
    pub fn forget(&self, installation: InstallationId) {
        self.entries
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&installation);
    }

    /// Drop every token held.
    pub fn clear(&self) {
        self.entries
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::application::fixtures::at;
    use abnegate_secret::SecretValue;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    /// An instant well after the Unix epoch.
    const NOW: u64 = 1_700_000_000;

    /// One minute.
    const MINUTE: Duration = Duration::from_mins(1);

    fn token(value: &str, minutes: u32) -> InstallationToken {
        InstallationToken::new(SecretValue::new(value), at(NOW) + MINUTE * minutes)
    }

    fn one() -> InstallationId {
        InstallationId::new(1)
    }

    fn two() -> InstallationId {
        InstallationId::new(2)
    }

    fn held(cache: &TokenCache, installation: InstallationId) -> String {
        cache
            .get(installation, at(NOW))
            .unwrap()
            .secret()
            .expose()
            .to_string()
    }

    #[test]
    fn an_empty_cache_holds_nothing() {
        assert!(TokenCache::new().get(one(), at(NOW)).is_none());
    }

    #[test]
    fn a_cached_token_is_returned_again() {
        let cache = TokenCache::new();
        cache.insert(one(), token("ghs_first", 60));

        assert_eq!(held(&cache, one()), "ghs_first");
        assert_eq!(held(&cache, one()), "ghs_first");
    }

    #[test]
    fn a_token_near_expiry_is_withheld_rather_than_served() {
        let cache = TokenCache::new();
        cache.insert(one(), token("ghs_nearly_spent", 60));

        assert!(
            cache.get(one(), at(NOW)).is_some(),
            "a fresh token is served"
        );
        assert!(
            cache.get(one(), at(NOW) + MINUTE * 56).is_none(),
            "four minutes of life is inside the safety margin"
        );
    }

    #[test]
    fn installations_do_not_share_a_token() {
        let cache = TokenCache::new();
        cache.insert(one(), token("ghs_one", 60));
        cache.insert(two(), token("ghs_two", 60));

        assert_eq!(held(&cache, one()), "ghs_one");
        assert_eq!(held(&cache, two()), "ghs_two");
    }

    #[test]
    fn a_replacement_supersedes_the_token_it_replaces() {
        let cache = TokenCache::new();
        cache.insert(one(), token("ghs_old", 60));
        cache.insert(one(), token("ghs_new", 60));

        assert_eq!(held(&cache, one()), "ghs_new");
    }

    #[test]
    fn forgetting_and_clearing_empty_the_cache() {
        let cache = TokenCache::new();
        cache.insert(one(), token("ghs_one", 60));
        cache.insert(two(), token("ghs_two", 60));

        cache.forget(one());
        assert!(cache.get(one(), at(NOW)).is_none());
        assert!(cache.get(two(), at(NOW)).is_some());

        cache.clear();
        assert!(cache.get(two(), at(NOW)).is_none());
    }

    #[test]
    fn the_cache_never_renders_the_tokens_it_holds() {
        let cache = TokenCache::new();
        cache.insert(one(), token("ghs_secret_value", 60));

        let rendered = format!("{cache:?}");
        assert!(!rendered.contains("ghs_secret_value"));
        assert!(rendered.contains("[REDACTED]"));
    }

    #[test]
    fn a_poisoned_cache_still_holds_and_serves_tokens() {
        let cache = Arc::new(TokenCache::new());
        cache.insert(one(), token("ghs_before", 60));
        let poisoner = Arc::clone(&cache);
        let panicked = thread::spawn(move || {
            let _guard = poisoner.entries.write().unwrap();
            panic!("poison the lock");
        })
        .join();
        assert!(panicked.is_err());
        assert!(cache.entries.is_poisoned());

        assert_eq!(held(&cache, one()), "ghs_before");
        cache.insert(two(), token("ghs_after", 60));
        assert_eq!(held(&cache, two()), "ghs_after");
        cache.forget(one());
        assert!(cache.get(one(), at(NOW)).is_none());
        cache.clear();
        assert!(cache.get(two(), at(NOW)).is_none());
    }
}
