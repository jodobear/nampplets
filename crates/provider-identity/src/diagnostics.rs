use std::{fmt, sync::Arc};

use nmp_native_nap_bridge::ProviderPushError;
use nmp_native_runtime_core::{BoundedJson, Principal, SessionId};

pub trait IdentityDiagnosticsSink: Send + Sync + fmt::Debug {
    fn record(&self, fact: IdentityDiagnostic);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdentityDiagnostic {
    Read {
        principal: Principal,
        session: SessionId,
        action: Arc<str>,
        frozen_pubkey: Option<Arc<str>>,
        scoped_evidence: BoundedJson,
    },
    PushRefused {
        principal: Principal,
        session: SessionId,
        reason: ProviderPushError,
    },
    ObservationClosed,
}

#[derive(Debug, Default)]
pub struct NoopIdentityDiagnostics;

impl IdentityDiagnosticsSink for NoopIdentityDiagnostics {
    fn record(&self, _fact: IdentityDiagnostic) {}
}
