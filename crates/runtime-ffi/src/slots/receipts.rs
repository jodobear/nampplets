//! Typed durable-receipt latest-state slot.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use nmp_native_runtime_app::{AppSnapshot, AppTerminalReason, SnapshotSection};

use super::SlotHub;
use crate::{RuntimeController, RuntimeReceiptSnapshot, RuntimeRefusal, project_receipt};

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RuntimeReceiptsSlotSnapshot {
    pub revision: u64,
    pub closed: bool,
    pub receipts: Vec<RuntimeReceiptSnapshot>,
}

/// A receipt slot either carries one authoritative latest state or a typed
/// terminal refusal. A revision-exhausted producer never wraps or saturates.
#[derive(Clone, Debug, uniffi::Enum)]
pub enum RuntimeReceiptsSlotProjection {
    Snapshot {
        snapshot: RuntimeReceiptsSlotSnapshot,
    },
    Refused {
        revision: u64,
        closed: bool,
        refusal: RuntimeRefusal,
    },
}

impl RuntimeReceiptsSlotProjection {
    pub(crate) fn refusal(&self) -> Option<RuntimeRefusal> {
        match self {
            Self::Snapshot { .. } => None,
            Self::Refused { refusal, .. } => Some(refusal.clone()),
        }
    }
}

#[uniffi::export(callback_interface)]
pub trait RuntimeReceiptsSlotObserver: Send + Sync {
    fn update(&self, projection: RuntimeReceiptsSlotProjection);
}

#[derive(Debug, uniffi::Object)]
pub struct RuntimeReceiptsSlotObservation {
    pub(crate) hub: Arc<SlotHub>,
    pub(crate) id: u64,
    pub(crate) stopped: AtomicBool,
}

#[uniffi::export]
impl RuntimeReceiptsSlotObservation {
    pub fn stop(&self) {
        if !self.stopped.swap(true, Ordering::AcqRel) {
            self.hub.remove_receipts(self.id);
        }
    }
}

impl Drop for RuntimeReceiptsSlotObservation {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeReceiptsSlotObservationStart {
    pub observation: Option<Arc<RuntimeReceiptsSlotObservation>>,
    pub initial: Option<RuntimeReceiptsSlotProjection>,
    pub refusal: Option<RuntimeRefusal>,
}

impl RuntimeReceiptsSlotObservationStart {
    pub(crate) fn refused(refusal: RuntimeRefusal) -> Self {
        Self {
            observation: None,
            initial: None,
            refusal: Some(refusal),
        }
    }
}

pub(crate) fn project_receipts(
    controller: &RuntimeController,
    source: &AppSnapshot,
) -> RuntimeReceiptsSlotProjection {
    if matches!(
        source.terminal_reason,
        Some(AppTerminalReason::SectionRevisionExhausted {
            section: SnapshotSection::Receipts
        })
    ) {
        return RuntimeReceiptsSlotProjection::Refused {
            revision: source.revisions.receipts,
            closed: true,
            refusal: controller.refusal(
                "receipts-slot-revision-exhausted",
                "the receipt section revision is exhausted",
            ),
        };
    }
    RuntimeReceiptsSlotProjection::Snapshot {
        snapshot: RuntimeReceiptsSlotSnapshot {
            revision: source.revisions.receipts,
            closed: source.closed,
            receipts: source.receipts.iter().map(project_receipt).collect(),
        },
    }
}
