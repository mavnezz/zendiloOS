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
}

impl LoginThrottle {
    pub fn new(policy: ThrottlePolicy) -> Self {
        Self { policy, failures: HashMap::new() }
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

    /// `Err(retry_after)` if `key` has no attempts left right now.
    pub fn check(&self, key: &ThrottleKey, now: Timestamp) -> Result<(), Duration> {
        let Some(times) = self.failures.get(key) else { return Ok(()) };
        let start = self.window_start(now);
        let recent = times.iter().filter(|t| **t > start);
        if recent.clone().count() < self.limit(key) {
            return Ok(());
        }
        // Blocked until the oldest recent failure leaves the window.
        let oldest = recent.min().copied().unwrap_or(now);
        Err(oldest.saturating_add(self.policy.window).duration_since(now).max(Duration::from_secs(1)))
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

    /// Forgets the failures of `key`, e.g. after a successful login.
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
