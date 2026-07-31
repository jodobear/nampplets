//! One ordered cut of the Rust-owned app snapshot and event stream.

use nmp_native_runtime_app::AppObserver;

use super::RuntimeController;
use crate::{RuntimeEvent, RuntimeSnapshotProjection, projection::project_event};

pub(crate) struct AppObservationCut {
    pub(crate) snapshot: RuntimeSnapshotProjection,
    pub(crate) events: Vec<RuntimeEvent>,
    pub(crate) oldest_available_event: u64,
    pub(crate) newest_available_event: u64,
    pub(crate) event_cursor_was_stale: bool,
    pub(crate) lost_before_batch: u64,
}

impl RuntimeController {
    /// Capture the snapshot before reading events. Stop writes terminal events
    /// under the app state lock and publishes its absent-session snapshot only
    /// afterwards. This ordering therefore either keeps the session in the
    /// snapshot or puts its terminal events in this batch (with loss explicit).
    pub(super) fn capture_app_observation(
        &self,
        observer: &AppObserver,
        event_cursor: u64,
    ) -> AppObservationCut {
        self.capture_app_observation_after_snapshot(observer, event_cursor, || {})
    }

    fn capture_app_observation_after_snapshot_inner(
        &self,
        observer: &AppObserver,
        event_cursor: u64,
        after_snapshot: impl FnOnce(),
    ) -> AppObservationCut {
        let source = observer.latest();
        after_snapshot();
        let batch = self.app.events_after(event_cursor);
        AppObservationCut {
            snapshot: self.project_snapshot(&source),
            events: batch
                .events
                .into_iter()
                .map(|event| project_event(event.sequence, &event.event))
                .collect(),
            oldest_available_event: batch.oldest_available,
            newest_available_event: batch.newest_available,
            event_cursor_was_stale: batch.cursor_was_stale,
            lost_before_batch: batch.lost_before_batch,
        }
    }

    #[cfg(test)]
    pub(crate) fn capture_app_observation_after_snapshot(
        &self,
        observer: &AppObserver,
        event_cursor: u64,
        after_snapshot: impl FnOnce(),
    ) -> AppObservationCut {
        self.capture_app_observation_after_snapshot_inner(observer, event_cursor, after_snapshot)
    }

    #[cfg(not(test))]
    fn capture_app_observation_after_snapshot(
        &self,
        observer: &AppObserver,
        event_cursor: u64,
        after_snapshot: impl FnOnce(),
    ) -> AppObservationCut {
        self.capture_app_observation_after_snapshot_inner(observer, event_cursor, after_snapshot)
    }
}
