use std::{fmt, sync::Arc};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Principal, SessionId};

/// Serialized schema value whose byte limit was checked before crossing the
/// runtime/adapter boundary.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BoundedJson(Arc<str>);

impl BoundedJson {
    pub fn from_value(
        value: &serde_json::Value,
        maximum_bytes: usize,
    ) -> Result<Self, BoundedJsonError> {
        let encoded = serde_json::to_string(value)
            .map_err(|error| BoundedJsonError::Invalid(error.to_string()))?;
        Self::from_raw(encoded, maximum_bytes)
    }

    pub fn from_raw(
        encoded: impl Into<String>,
        maximum_bytes: usize,
    ) -> Result<Self, BoundedJsonError> {
        let encoded = encoded.into();
        if encoded.len() > maximum_bytes {
            return Err(BoundedJsonError::TooLarge {
                actual: encoded.len(),
                maximum: maximum_bytes,
            });
        }
        serde_json::from_str::<serde_json::Value>(&encoded)
            .map_err(|error| BoundedJsonError::Invalid(error.to_string()))?;
        Ok(Self(Arc::from(encoded)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn decode(&self) -> serde_json::Result<serde_json::Value> {
        serde_json::from_str(&self.0)
    }

    pub fn byte_len(&self) -> usize {
        self.0.len()
    }
}

impl fmt::Debug for BoundedJson {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundedJson")
            .field("byte_len", &self.byte_len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BoundedJsonError {
    #[error("JSON payload is {actual} bytes; the maximum is {maximum}")]
    TooLarge { actual: usize, maximum: usize },
    #[error("invalid JSON payload: {0}")]
    Invalid(String),
}

/// Host-defined binding request. Parameters have already passed the binding
/// family's schema validator; untrusted components cannot create this value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingRequest {
    pub workspace_binding_id: Arc<str>,
    pub family: Arc<str>,
    pub schema: Arc<str>,
    pub parameters: BoundedJson,
    pub maximum_rows: u32,
    pub maximum_frame_bytes: usize,
}

/// One authoritative source projection. It deliberately contains scoped
/// evidence rather than a global "synced" or "complete" flag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostBindingSnapshot {
    pub source_generation: u64,
    pub value: BoundedJson,
    pub scoped_evidence: BoundedJson,
}

/// Conflating sink contract: implementations replace their single pending
/// value with the newest snapshot; they must not enqueue every update.
pub trait BindingEventSink: Send + Sync + fmt::Debug {
    fn push_latest(&self, snapshot: HostBindingSnapshot) -> Result<(), BindingSinkError>;
    fn close(&self, reason: Option<Arc<str>>);
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum BindingSinkError {
    #[error("binding sink is closed")]
    Closed,
    #[error("binding snapshot exceeds the negotiated frame bound")]
    FrameTooLarge,
}

/// Exact NMP observation owner. `close` is deterministic and idempotent.
pub trait HostBindingHandle: Send + Sync + fmt::Debug {
    fn logical_id(&self) -> &str;
    fn close(&self);
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountRef(pub Arc<str>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovedWrite {
    pub approval_id: Arc<str>,
    pub origin_principal: Principal,
    pub origin_session: SessionId,
    /// Frozen account selected by native approval.
    pub account: AccountRef,
    /// Exact approved draft in the adapter's governed public-facade format.
    pub draft: BoundedJson,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WriteReceiptId(pub Arc<str>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceptedWrite {
    pub receipt_id: WriteReceiptId,
    pub frozen_account: AccountRef,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptSnapshot {
    pub receipt_id: WriteReceiptId,
    pub state: BoundedJson,
}

/// Receipt observation is app/runtime-profile owned, never component owned.
pub trait ReceiptEventSink: Send + Sync + fmt::Debug {
    fn push_latest(&self, snapshot: ReceiptSnapshot) -> Result<(), ReceiptSinkError>;
    fn close(&self, reason: Option<Arc<str>>);
}

/// One adapter-boundary receipt delivery owner.
///
/// [`ReceiptObservation::stop_delivery`] deterministically stops delivery to
/// the supplied runtime sink according to the selected public facade. A direct
/// Rust adapter can close/detach its `FifoReceiver`; a native boundary may only
/// be able to end app-side consumption. In every case, stopping delivery never
/// cancels or weakens the durable write obligation.
pub trait ReceiptObservation: Send + Sync + fmt::Debug {
    fn receipt_id(&self) -> &WriteReceiptId;
    fn stop_delivery(&self);
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ReceiptSinkError {
    #[error("receipt sink is closed")]
    Closed,
    #[error("receipt snapshot exceeds the negotiated frame bound")]
    FrameTooLarge,
}

#[derive(Debug)]
pub enum ReceiptReattachment {
    Attached(Arc<dyn ReceiptObservation>),
    NotFound,
}

/// Public-facade seam implemented by `nmp-adapter`.
///
/// No NMP type crosses this interface. The adapter retains canonical event,
/// query, routing, signer, and durable-write ownership. The runtime owns only
/// bounded host projections and exact receipt identifiers.
pub trait HostDataPlane: Send + Sync + fmt::Debug {
    fn open_binding(
        &self,
        request: BindingRequest,
        sink: Arc<dyn BindingEventSink>,
    ) -> Result<Arc<dyn HostBindingHandle>, HostDataError>;

    /// Acceptance transfers the durable obligation to NMP. It must occur only
    /// after exact native approval and must preserve the frozen account.
    fn accept_write(
        &self,
        approved: ApprovedWrite,
        receipt_sink: Arc<dyn ReceiptEventSink>,
    ) -> Result<AcceptedWrite, HostDataError>;

    fn reattach_receipt(
        &self,
        receipt_id: WriteReceiptId,
        receipt_sink: Arc<dyn ReceiptEventSink>,
    ) -> Result<ReceiptReattachment, HostDataError>;
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum HostDataError {
    #[error("{component} executor capacity {capacity} is full")]
    ExecutorSaturated {
        component: Arc<str>,
        capacity: usize,
    },
    #[error("{component} could not start an OS thread: {reason}")]
    ThreadUnavailable {
        component: Arc<str>,
        reason: Arc<str>,
    },
    #[error("host data service is closed")]
    ServiceClosed,
    #[error("host binding request was refused: {reason}")]
    BindingRefused { reason: Arc<str> },
    #[error("approved write was refused before acceptance: {reason}")]
    WriteRefused { reason: Arc<str> },
    #[error("retained receipt could not be read: {reason}")]
    ReceiptUnreadable { reason: Arc<str> },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_json_refuses_oversize_payload() {
        let value = serde_json::json!({"rows": [1, 2, 3]});
        assert!(matches!(
            BoundedJson::from_value(&value, 4),
            Err(BoundedJsonError::TooLarge { .. })
        ));
    }
}
