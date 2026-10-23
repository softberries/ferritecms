//! Named locks on SQLite (04 §7, P1): an in-process mutex per store. The data-folder lock
//! already keeps every other process off the database, so nothing else can contend.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};

use async_trait::async_trait;
use ferrite_store::{LockGuard, StoreError};
use tokio::sync::OwnedMutexGuard;

/// One mutex per lock name, created on first use.
#[derive(Default)]
pub(crate) struct Locks {
    by_name: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Locks {
    /// Takes `name` if free; the caller has validated the name.
    pub(crate) fn try_lock(&self, name: &str) -> Option<Box<dyn LockGuard>> {
        let mutex = self
            .by_name
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(name.to_owned())
            .or_default()
            .clone();
        let guard = mutex.try_lock_owned().ok()?;
        Some(Box::new(SqliteLockGuard { _held: guard }))
    }
}

/// Holds the mutex until released or dropped.
struct SqliteLockGuard {
    _held: OwnedMutexGuard<()>,
}

#[async_trait]
impl LockGuard for SqliteLockGuard {
    async fn release(self: Box<Self>) -> Result<(), StoreError> {
        drop(self);
        Ok(())
    }
}
