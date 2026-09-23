//! A value used by one caller at a time.

use std::sync::{Mutex, MutexGuard, PoisonError};

/// A mutex whose value outlives a panic in whoever held it.
///
/// Code that runs under the lock includes a caller's own closure, and a panic
/// there says nothing about the value guarded here, which is whole between
/// operations. Poisoning would turn one caller's bug into a panic in every
/// later call.
pub(crate) struct Exclusive<T>(Mutex<T>);

impl<T> Exclusive<T> {
    pub(crate) const fn new(value: T) -> Self {
        Self(Mutex::new(value))
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, T> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use super::*;

    #[test]
    fn a_panic_under_the_lock_does_not_shut_out_later_callers() {
        let shared = Exclusive::new(vec![1]);

        let panicked = catch_unwind(AssertUnwindSafe(|| {
            let mut value = shared.lock();
            value.push(2);
            panic!("a caller's closure failed while holding the lock");
        }));

        assert!(panicked.is_err());
        assert_eq!(*shared.lock(), vec![1, 2]);
    }
}
