//! Producer-side section identity for bounded latest-state publication.

use super::AppState;
use crate::views::{AppSnapshot, SectionRevisions, SnapshotSection};

pub(super) fn advance_revisions(
    previous: &AppSnapshot,
    current: &AppSnapshot,
    state: &AppState,
) -> Result<SectionRevisions, SnapshotSection> {
    fn step(
        previous: u64,
        changed: bool,
        section: SnapshotSection,
    ) -> Result<u64, SnapshotSection> {
        if changed {
            previous.checked_add(1).ok_or(section)
        } else {
            Ok(previous)
        }
    }

    let revisions = previous.revisions;
    Ok(SectionRevisions {
        library: step(
            revisions.library,
            current.library != previous.library,
            SnapshotSection::Library,
        )?,
        sessions: step(
            revisions.sessions,
            current.sessions != previous.sessions
                || current.session_domains != previous.session_domains,
            SnapshotSection::Sessions,
        )?,
        provider_push_lanes: step(
            revisions.provider_push_lanes,
            current.provider_push_lanes != previous.provider_push_lanes,
            SnapshotSection::ProviderPushLanes,
        )?,
        bindings: step(
            revisions.bindings,
            current.bindings != previous.bindings,
            SnapshotSection::Bindings,
        )?,
        pending_writes: step(
            revisions.pending_writes,
            current.pending_writes != previous.pending_writes,
            SnapshotSection::PendingWrites,
        )?,
        receipts: step(
            revisions.receipts,
            current.receipts != previous.receipts,
            SnapshotSection::Receipts,
        )?,
        workspaces: step(
            revisions.workspaces,
            current.workspaces != previous.workspaces,
            SnapshotSection::Workspaces,
        )?,
        resources: step(
            revisions.resources,
            current.resources != previous.resources,
            SnapshotSection::Resources,
        )?,
        activity: state.activity.appended(),
        errors: state.errors.appended(),
        newest_event_sequence: state.next_event_sequence,
    })
}
