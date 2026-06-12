use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use tracing::error;

pub(crate) fn read_lock<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    match lock.read() {
        Ok(guard) => guard,
        Err(poisoned) => {
            error!(target: "speech", "[lock] RwLock poisoned on read — recovering with potentially inconsistent data.");
            poisoned.into_inner()
        }
    }
}

pub(crate) fn write_lock<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    match lock.write() {
        Ok(guard) => guard,
        Err(poisoned) => {
            error!(target: "speech", "[lock] RwLock poisoned on write — recovering with potentially inconsistent data.");
            poisoned.into_inner()
        }
    }
}

pub(crate) fn mutex_lock<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    match lock.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            error!(target: "speech", "[lock] Mutex poisoned — recovering with potentially inconsistent data.");
            poisoned.into_inner()
        }
    }
}
