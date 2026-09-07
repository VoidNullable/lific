//! A process-wide lock for tests that set environment variables.
//!
//! `cargo test` runs every test in one process on many threads, and the
//! environment is shared. Tests that set `LIFIC_URL`/`LIFIC_TOKEN`, and tests
//! that depend on those being unset, must take the same lock or they fail each
//! other at random.

use tokio::sync::MutexGuard;

pub(crate) struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    restore: Vec<(String, Option<String>)>,
}

impl EnvGuard {
    /// Take the lock and apply `pairs`; `None` unsets. Previous values are
    /// restored on drop, including during a panic.
    pub(crate) fn set(pairs: &[(&str, Option<&str>)]) -> Self {
        let lock = crate::test_env::lock_lific_token_env_blocking();
        let mut restore = Vec::with_capacity(pairs.len());
        for (key, value) in pairs {
            restore.push(((*key).to_owned(), std::env::var(key).ok()));
            apply(key, *value);
        }
        Self {
            _lock: lock,
            restore,
        }
    }

    /// Take the lock and unset `keys`.
    pub(crate) fn cleared(keys: &[&str]) -> Self {
        let pairs: Vec<(&str, Option<&str>)> = keys.iter().map(|key| (*key, None)).collect();
        Self::set(&pairs)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.restore {
            apply(key, value.as_deref());
        }
    }
}

fn apply(key: &str, value: Option<&str>) {
    // SAFETY: all mutation of these variables in this test binary is serialized
    // by the shared test_env lock, held for the guard's lifetime.
    unsafe {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
}
