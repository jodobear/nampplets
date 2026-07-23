use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
    time::Instant,
};

use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Cancellation, Capability, SessionId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceClass {
    ProviderCall,
    Subscription,
    ResourceStream,
    StateDelivery,
    Action,
    WebView,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceLimits {
    pub global: usize,
    pub per_session: usize,
    pub per_class: BTreeMap<ResourceClass, usize>,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            global: 128,
            per_session: 24,
            per_class: BTreeMap::from([
                (ResourceClass::ProviderCall, 32),
                (ResourceClass::Subscription, 32),
                (ResourceClass::ResourceStream, 8),
                (ResourceClass::StateDelivery, 32),
                (ResourceClass::Action, 16),
                (ResourceClass::WebView, 8),
            ]),
        }
    }
}

#[derive(Debug)]
pub struct ResourceTracker {
    inner: Arc<TrackerInner>,
}

#[derive(Debug)]
struct TrackerInner {
    limits: ResourceLimits,
    state: Mutex<TrackerState>,
    changed: Condvar,
}

#[derive(Debug, Default)]
struct TrackerState {
    next_id: u64,
    work: BTreeMap<u64, WorkEntry>,
    by_session: BTreeMap<SessionId, usize>,
    by_class: BTreeMap<ResourceClass, usize>,
    high_watermark: usize,
    refusal_count: u64,
}

#[derive(Debug)]
struct WorkEntry {
    session: SessionId,
    capability: Option<Capability>,
    class: ResourceClass,
    cancellation: Weak<crate::cancellation::Inner>,
}

/// An admitted unit of native work. Dropping the last lease returns its permit.
#[derive(Debug)]
pub struct WorkLease {
    registration: Option<LeaseRegistration>,
    cancellation: Cancellation,
}

#[derive(Debug)]
struct LeaseRegistration {
    id: u64,
    tracker: Weak<TrackerInner>,
}

impl ResourceTracker {
    pub fn new(limits: ResourceLimits) -> Result<Self, ResourceRefusal> {
        if limits.global == 0 || limits.per_session == 0 {
            return Err(ResourceRefusal::InvalidLimits);
        }
        for class in [
            ResourceClass::ProviderCall,
            ResourceClass::Subscription,
            ResourceClass::ResourceStream,
            ResourceClass::StateDelivery,
            ResourceClass::Action,
            ResourceClass::WebView,
        ] {
            if limits.per_class.get(&class).copied().unwrap_or(0) == 0 {
                return Err(ResourceRefusal::InvalidLimits);
            }
        }
        Ok(Self {
            inner: Arc::new(TrackerInner {
                limits,
                state: Mutex::new(TrackerState::default()),
                changed: Condvar::new(),
            }),
        })
    }

    pub fn admit(
        &self,
        session: SessionId,
        capability: Option<Capability>,
        class: ResourceClass,
    ) -> Result<WorkLease, ResourceRefusal> {
        let cancellation = Cancellation::new();
        let cancellation_weak = cancellation.registry_weak();
        let mut state = self.inner.state.lock();
        let class_limit = self.inner.limits.per_class[&class];
        let session_count = state.by_session.get(&session).copied().unwrap_or_default();
        let class_count = state.by_class.get(&class).copied().unwrap_or_default();

        let refusal = if state.work.len() >= self.inner.limits.global {
            Some(ResourceRefusal::GlobalCapacity {
                capacity: self.inner.limits.global,
            })
        } else if session_count >= self.inner.limits.per_session {
            Some(ResourceRefusal::SessionCapacity {
                session,
                capacity: self.inner.limits.per_session,
            })
        } else if class_count >= class_limit {
            Some(ResourceRefusal::ClassCapacity {
                class,
                capacity: class_limit,
            })
        } else {
            None
        };

        if let Some(refusal) = refusal {
            state.refusal_count = state.refusal_count.saturating_add(1);
            return Err(refusal);
        }

        state.next_id = state.next_id.wrapping_add(1).max(1);
        let id = state.next_id;
        state.work.insert(
            id,
            WorkEntry {
                session,
                capability,
                class,
                cancellation: cancellation_weak,
            },
        );
        *state.by_session.entry(session).or_default() += 1;
        *state.by_class.entry(class).or_default() += 1;
        state.high_watermark = state.high_watermark.max(state.work.len());

        Ok(WorkLease {
            registration: Some(LeaseRegistration {
                id,
                tracker: Arc::downgrade(&self.inner),
            }),
            cancellation,
        })
    }

    pub fn cancel_session(&self, session: SessionId) -> usize {
        let state = self.inner.state.lock();
        let signals: Vec<_> = state
            .work
            .values()
            .filter(|entry| entry.session == session)
            .filter_map(|entry| entry.cancellation.upgrade())
            .collect();
        drop(state);
        for signal in &signals {
            signal.cancel();
        }
        signals.len()
    }

    pub fn cancel_capability(&self, capability: &Capability) -> usize {
        let state = self.inner.state.lock();
        let signals: Vec<_> = state
            .work
            .values()
            .filter(|entry| entry.capability.as_ref() == Some(capability))
            .filter_map(|entry| entry.cancellation.upgrade())
            .collect();
        drop(state);
        for signal in &signals {
            signal.cancel();
        }
        signals.len()
    }

    pub fn cancel_session_capability(&self, session: SessionId, capability: &Capability) -> usize {
        let state = self.inner.state.lock();
        let signals: Vec<_> = state
            .work
            .values()
            .filter(|entry| {
                entry.session == session && entry.capability.as_ref() == Some(capability)
            })
            .filter_map(|entry| entry.cancellation.upgrade())
            .collect();
        drop(state);
        for signal in &signals {
            signal.cancel();
        }
        signals.len()
    }

    pub fn census(&self) -> ResourceCensus {
        let state = self.inner.state.lock();
        ResourceCensus {
            admitted: state.work.len(),
            by_session: state.by_session.clone(),
            by_class: state.by_class.clone(),
            high_watermark: state.high_watermark,
            refusal_count: state.refusal_count,
        }
    }

    /// Waits on permit release notifications rather than polling.
    pub fn wait_for_idle(&self, deadline: Instant) -> Result<(), ResourceRefusal> {
        let mut state = self.inner.state.lock();
        while !state.work.is_empty() {
            let now = Instant::now();
            if now >= deadline {
                return Err(ResourceRefusal::IdleDeadline {
                    remaining: state.work.len(),
                });
            }
            self.inner
                .changed
                .wait_for(&mut state, deadline.saturating_duration_since(now));
        }
        Ok(())
    }
}

impl Clone for ResourceTracker {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl WorkLease {
    pub fn cancellation(&self) -> &Cancellation {
        &self.cancellation
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}

impl Drop for WorkLease {
    fn drop(&mut self) {
        let Some(registration) = self.registration.take() else {
            return;
        };
        let Some(tracker) = registration.tracker.upgrade() else {
            return;
        };
        let mut state = tracker.state.lock();
        let Some(entry) = state.work.remove(&registration.id) else {
            return;
        };
        decrement_or_remove(&mut state.by_session, &entry.session);
        decrement_or_remove(&mut state.by_class, &entry.class);
        tracker.changed.notify_all();
    }
}

fn decrement_or_remove<K: Ord + Clone>(counts: &mut BTreeMap<K, usize>, key: &K) {
    if let Some(count) = counts.get_mut(key) {
        *count -= 1;
        if *count == 0 {
            counts.remove(key);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceCensus {
    pub admitted: usize,
    pub by_session: BTreeMap<SessionId, usize>,
    pub by_class: BTreeMap<ResourceClass, usize>,
    pub high_watermark: usize,
    pub refusal_count: u64,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ResourceRefusal {
    #[error("resource limits must all be finite and non-zero")]
    InvalidLimits,
    #[error("global runtime work capacity {capacity} is full")]
    GlobalCapacity { capacity: usize },
    #[error("session {session:?} work capacity {capacity} is full")]
    SessionCapacity { session: SessionId, capacity: usize },
    #[error("{class:?} capacity {capacity} is full")]
    ClassCapacity {
        class: ResourceClass,
        capacity: usize,
    },
    #[error("resource idle deadline elapsed with {remaining} work items active")]
    IdleDeadline { remaining: usize },
}

#[cfg(test)]
mod tests {
    use std::{thread, time::Duration};

    use super::*;

    #[test]
    fn capacity_refusal_is_observable_and_other_session_remains_usable() {
        let tracker = ResourceTracker::new(ResourceLimits {
            global: 2,
            per_session: 1,
            per_class: BTreeMap::from([
                (ResourceClass::ProviderCall, 2),
                (ResourceClass::Subscription, 2),
                (ResourceClass::ResourceStream, 2),
                (ResourceClass::StateDelivery, 2),
                (ResourceClass::Action, 2),
                (ResourceClass::WebView, 2),
            ]),
        })
        .unwrap();
        let first = tracker
            .admit(SessionId(1), None, ResourceClass::ProviderCall)
            .unwrap();
        assert!(matches!(
            tracker.admit(SessionId(1), None, ResourceClass::ProviderCall),
            Err(ResourceRefusal::SessionCapacity {
                session: SessionId(1),
                capacity: 1,
            })
        ));
        let second = tracker
            .admit(SessionId(2), None, ResourceClass::ProviderCall)
            .unwrap();
        assert_eq!(tracker.census().admitted, 2);
        drop((first, second));
        assert_eq!(tracker.census().admitted, 0);
    }

    #[test]
    fn cancellation_and_idle_barrier_are_event_driven() {
        let tracker = ResourceTracker::new(ResourceLimits::default()).unwrap();
        let capability = Capability::new("resource").unwrap();
        let lease = tracker
            .admit(
                SessionId(1),
                Some(capability.clone()),
                ResourceClass::ResourceStream,
            )
            .unwrap();
        let cancellation = lease.cancellation().clone();
        let worker = thread::spawn(move || {
            cancellation.wait();
            drop(lease);
        });

        assert_eq!(tracker.cancel_capability(&capability), 1);
        tracker
            .wait_for_idle(Instant::now() + Duration::from_secs(1))
            .unwrap();
        worker.join().unwrap();
        assert_eq!(tracker.census().admitted, 0);
    }
}
