use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

type FlightKey = [u8; 32];
type FlightMap = Arc<Mutex<HashMap<FlightKey, Weak<AsyncMutex<()>>>>>;

#[derive(Clone, Default)]
pub(super) struct SingleFlight {
    entries: FlightMap,
}

impl SingleFlight {
    pub async fn enter(&self, key: FlightKey) -> FlightPermit {
        let lock = {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            entries
                .get(&key)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| {
                    let lock = Arc::new(AsyncMutex::new(()));
                    entries.insert(key, Arc::downgrade(&lock));
                    lock
                })
        };
        let registration = FlightRegistration {
            entries: self.entries.clone(),
            key,
            lock,
        };
        let guard = registration.lock.clone().lock_owned().await;
        FlightPermit {
            _guard: guard,
            _registration: registration,
        }
    }
}

struct FlightRegistration {
    entries: FlightMap,
    key: FlightKey,
    lock: Arc<AsyncMutex<()>>,
}

impl Drop for FlightRegistration {
    fn drop(&mut self) {
        if Arc::strong_count(&self.lock) > 2 {
            return;
        }
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if entries
            .get(&self.key)
            .is_some_and(|stored| Weak::ptr_eq(stored, &Arc::downgrade(&self.lock)))
        {
            entries.remove(&self.key);
        }
    }
}

pub(super) struct FlightPermit {
    _guard: OwnedMutexGuard<()>,
    _registration: FlightRegistration,
}
