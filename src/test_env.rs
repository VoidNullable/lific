//! LIF-401: one process, one environment, one lock.
//!
//! The unit-test binary runs ~1800 tests in a single process, and a handful
//! of them mutate or read the `LIFIC_TOKEN` environment variable. They used
//! to serialize on two *module-local* mutexes (`cli::credentials` and
//! `auth`) that never serialized against each other — concrete interleavings
//! existed where a `remove_var` in one module landed between another
//! module's `set_var` and read (UB on glibc: environ can be reallocated under
//! the reader). This module is the single lock they all take. Doctor's
//! offline path no longer needs the lock because it probes reachability
//! before attempting credential lookup.
//!
//! All current callers are synchronous and use `blocking_lock()`. tokio's
//! `Mutex` rather than `std`'s is used because it does not poison: a test that
//! panics while holding the lock must not cascade into every later env test
//! failing on `PoisonError`.

use tokio::sync::{Mutex, MutexGuard};

static LIFIC_TOKEN_ENV: Mutex<()> = Mutex::const_new(());

/// Take the process-wide `LIFIC_TOKEN` lock from a synchronous test. Every
/// test that calls `set_var`/`remove_var` on `LIFIC_TOKEN`, or exercises a
/// production path that reads it, must hold this guard for the whole
/// mutate-read-restore sequence.
pub(crate) fn lock_lific_token_env_blocking() -> MutexGuard<'static, ()> {
    LIFIC_TOKEN_ENV.blocking_lock()
}
