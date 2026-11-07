use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::time::Duration;

use zendiloos_kernel::Timestamp;

use super::AccountName;

/// What failed attempts are counted against.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ThrottleKey {
    Address(IpAddr),
    Account(AccountName),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThrottlePolicy {
    /// Failures older than this no longer count.
    pub window: Duration,
    /// Protects a single account against guessing from many addresses.
    pub max_failures_per_account: usize,
    /// Protects all accounts against guessing from one address. Higher,
    /// since several people may share an address.
    pub max_failures_per_address: usize,
    /// Bounds memory when many addresses fail.
    pub max_tracked_addresses: usize,
}

impl Default for ThrottlePolicy {
    fn default() -> Self {
        Self {
            window: Duration::from_secs(15 * 60),
            max_failures_per_account: 5,
            max_failures_per_address: 20,
            max_tracked_addresses: 10_000,
        }
    }
}

/// Limits failed logins (and other secret checks) per account and per
/// address within a sliding window.
///
/// Kept in memory: a restart resets it, which is acceptable for a rate
/// limit, and avoids a database write for every failed attempt.
#[derive(Debug)]
pub struct LoginThrottle {
    policy: ThrottlePolicy,
    /// Times of recent failures per key, oldest first.
    failures: HashMap<ThrottleKey, VecDeque<Timestamp>>,
    /// Attempts under way per key: each counts as a failure until it ends,
    /// so attempts made at once cannot all pass while none has failed yet.
    under_way: HashMap<ThrottleKey, usize>,
}

impl LoginThrottle {
    pub fn new(policy: ThrottlePolicy) -> Self {
        Self { policy, failures: HashMap::new(), under_way: HashMap::new() }
    }

    fn limit(&self, key: &ThrottleKey) -> usize {
        match key {
            ThrottleKey::Address(_) => self.policy.max_failures_per_address,
            ThrottleKey::Account(_) => self.policy.max_failures_per_account,
        }
    }

    fn window_start(&self, now: Timestamp) -> Timestamp {
        now.saturating_sub(self.policy.window)
    }

    /// The failures of `key` within the window.
    fn recent(&self, key: &ThrottleKey, now: Timestamp) -> impl Iterator<Item = Timestamp> + Clone + '_ {
        let start = self.window_start(now);
        self.failures.get(key).into_iter().flatten().copied().filter(move |t| *t > start)
    }

    /// `Err(retry_after)` if `key` has no attempts left right now, counting
    /// those under way as failed.
    pub fn check(&self, key: &ThrottleKey, now: Timestamp) -> Result<(), Duration> {
        let recent = self.recent(key, now);
        let under_way = self.under_way.get(key).copied().unwrap_or(0);
        if recent.clone().count() + under_way < self.limit(key) {
            return Ok(());
        }
        // Blocked until the oldest recent failure leaves the window; one
        // still under way would fail now.
        let oldest = recent.min().unwrap_or(now);
        Err(oldest.saturating_add(self.policy.window).duration_since(now).max(Duration::from_secs(1)))
    }

    /// Whether the failures of `key` alone reached the limit, not counting
    /// attempts still under way, which may yet succeed.
    pub fn reached(&self, key: &ThrottleKey, now: Timestamp) -> bool {
        self.recent(key, now).count() >= self.limit(key)
    }

    /// Counts an attempt for `key` as under way, once [`check`](Self::check)
    /// let it through.
    pub fn start(&mut self, key: &ThrottleKey) {
        *self.under_way.entry(key.clone()).or_default() += 1;
    }

    /// Ends an attempt [`start`](Self::start) counted; a failed one is
    /// recorded.
    pub fn end(&mut self, key: &ThrottleKey, failed: bool, now: Timestamp) {
        if let Some(count) = self.under_way.get_mut(key) {
            *count -= 1;
            if *count == 0 {
                self.under_way.remove(key);
            }
        }
        if failed {
            self.record_failure(key.clone(), now);
        }
    }

    pub fn record_failure(&mut self, key: ThrottleKey, now: Timestamp) {
        let start = self.window_start(now);
        let limit = self.limit(&key);
        let times = self.failures.entry(key).or_default();
        times.retain(|t| *t > start);
        times.push_back(now);
        // Only the newest `limit` failures can ever matter.
        while times.len() > limit {
            times.pop_front();
        }
        self.bound_memory(now);
    }

    /// Forgets the failures of `key`, e.g. after a successful login; not
    /// the attempts still under way.
    pub fn clear(&mut self, key: &ThrottleKey) {
        self.failures.remove(key);
    }

    fn bound_memory(&mut self, now: Timestamp) {
        let addresses = |failures: &HashMap<ThrottleKey, _>| {
            failures.keys().filter(|k| matches!(k, ThrottleKey::Address(_))).count()
        };
        if addresses(&self.failures) <= self.policy.max_tracked_addresses {
            return;
        }
        let start = self.window_start(now);
        self.failures.retain(|_, times| times.back().is_some_and(|t| *t > start));
        // Still too many: drop the addresses that failed longest ago. Account
        // keys are never dropped; there are only as many as real accounts.
        while addresses(&self.failures) > self.policy.max_tracked_addresses {
            let stalest = self
                .failures
                .iter()
                .filter(|(k, _)| matches!(k, ThrottleKey::Address(_)))
                .min_by_key(|(_, times)| times.back().copied())
                .map(|(k, _)| k.clone());
            match stalest {
                Some(key) => self.failures.remove(&key),
                None => break,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_unix_seconds(seconds)
    }

    fn account(name: &str) -> ThrottleKey {
        ThrottleKey::Account(AccountName::parse(name).unwrap())
    }

    fn address(last: u8) -> ThrottleKey {
        ThrottleKey::Address(IpAddr::from([192, 0, 2, last]))
    }

    #[test]
    fn blocks_an_account_after_five_failures_until_the_oldest_expires() {
        let mut throttle = LoginThrottle::new(ThrottlePolicy::default());
        for t in 0..5 {
            assert_eq!(throttle.check(&account("alice"), at(1000 + t)), Ok(()));
            throttle.record_failure(account("alice"), at(1000 + t));
        }
        // The oldest failure (t = 1000) leaves the 15-minute window at 1900.
        assert_eq!(throttle.check(&account("alice"), at(1010)), Err(Duration::from_secs(890)));
        assert_eq!(throttle.check(&account("alice"), at(1900)), Ok(()));
        assert_eq!(throttle.check(&account("bob"), at(1010)), Ok(()), "other accounts are unaffected");
    }

    #[test]
    fn addresses_get_more_attempts_than_accounts() {
        let mut throttle = LoginThrottle::new(ThrottlePolicy::default());
        for t in 0..19 {
            throttle.record_failure(address(1), at(t));
        }
        assert_eq!(throttle.check(&address(1), at(20)), Ok(()));
        throttle.record_failure(address(1), at(20));
        assert!(throttle.check(&address(1), at(21)).is_err());
    }

    #[test]
    fn clearing_forgets_failures() {
        let mut throttle = LoginThrottle::new(ThrottlePolicy::default());
        for t in 0..5 {
            throttle.record_failure(account("alice"), at(t));
        }
        throttle.clear(&account("alice"));
        assert_eq!(throttle.check(&account("alice"), at(6)), Ok(()));
    }

    #[test]
    fn attempts_under_way_count_as_failures_until_they_end() {
        let mut throttle = LoginThrottle::new(ThrottlePolicy::default());
        throttle.record_failure(account("alice"), at(0));
        for _ in 0..4 {
            assert_eq!(throttle.check(&account("alice"), at(10)), Ok(()));
            throttle.start(&account("alice"));
        }

        assert_eq!(throttle.check(&account("alice"), at(10)), Err(Duration::from_secs(890)), "five at once");
        assert!(!throttle.reached(&account("alice"), at(10)), "those under way may yet succeed");
        throttle.end(&account("alice"), false, at(11));
        assert_eq!(throttle.check(&account("alice"), at(11)), Ok(()), "one succeeded");
        throttle.clear(&account("alice"));
        for _ in 0..3 {
            throttle.end(&account("alice"), true, at(12));
        }
        assert!(throttle.check(&account("alice"), at(12)).is_ok(), "three failures");
        throttle.start(&account("alice"));
        throttle.start(&account("alice"));
        assert!(throttle.check(&account("alice"), at(12)).is_err(), "clearing kept those under way");
        throttle.end(&account("alice"), true, at(13));
        throttle.end(&account("alice"), true, at(13));
        assert!(throttle.reached(&account("alice"), at(13)));
        assert!(throttle.under_way.is_empty());
    }

    #[test]
    fn under_way_alone_blocks_for_a_whole_window() {
        let mut throttle = LoginThrottle::new(ThrottlePolicy::default());
        for _ in 0..5 {
            throttle.start(&account("alice"));
        }
        assert_eq!(throttle.check(&account("alice"), at(0)), Err(Duration::from_secs(900)));
    }

    #[test]
    fn ending_an_attempt_never_counted_records_only_a_failure() {
        let mut throttle = LoginThrottle::new(ThrottlePolicy::default());
        throttle.end(&account("alice"), false, at(0));
        assert!(throttle.failures.is_empty() && throttle.under_way.is_empty());
        throttle.end(&account("alice"), true, at(0));
        assert_eq!(throttle.recent(&account("alice"), at(0)).count(), 1);
    }

    #[test]
    fn tracks_a_bounded_number_of_addresses_but_every_account() {
        let policy = ThrottlePolicy { max_tracked_addresses: 3, ..ThrottlePolicy::default() };
        let mut throttle = LoginThrottle::new(policy);
        for t in 0..5 {
            throttle.record_failure(account("alice"), at(t));
        }
        for last in 1..=5u8 {
            throttle.record_failure(address(last), at(i64::from(last) + 10));
        }
        let addresses = throttle.failures.keys().filter(|k| matches!(k, ThrottleKey::Address(_))).count();
        assert_eq!(addresses, 3);
        assert!(throttle.failures.contains_key(&address(5)), "the newest address is kept");
        assert!(throttle.check(&account("alice"), at(20)).is_err(), "account keys survive");
    }
}
