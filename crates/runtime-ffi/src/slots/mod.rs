//! Additive, concern-specific latest-state observation slots.
//!
//! Slots share one lazy controller-owned worker. Each slot owns its projection
//! cadence, bounded registry, typed refusal semantics, and cancellation handle;
//! relay diagnostics remains an independent NMP-owned observation surface.

mod control;
mod hub;
mod receipts;

pub(crate) use hub::SlotHub;
pub(crate) use receipts::project_receipts;
pub use receipts::{
    RuntimeReceiptsSlotObservation, RuntimeReceiptsSlotObservationStart,
    RuntimeReceiptsSlotObserver, RuntimeReceiptsSlotProjection, RuntimeReceiptsSlotSnapshot,
};
