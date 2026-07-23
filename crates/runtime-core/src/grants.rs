use std::{collections::BTreeMap, fmt, sync::Arc};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Principal, ResourceTracker, SessionId};

const MAX_CAPABILITY_BYTES: usize = 64;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Capability(String);

impl Capability {
    pub fn new(value: impl Into<String>) -> Result<Self, GrantError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_CAPABILITY_BYTES
            || !value.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'-' | b'_')
            })
        {
            return Err(GrantError::InvalidCapability);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Capability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Capability").field(&self.0).finish()
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantDecision {
    Denied,
    AskEveryTime,
    AllowSession,
    AllowExactBuild,
    Managed,
}

impl GrantDecision {
    pub fn allows_without_prompt(self) -> bool {
        matches!(
            self,
            Self::AllowSession | Self::AllowExactBuild | Self::Managed
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    Ordinary,
    Sensitive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GrantLimits {
    pub principals: usize,
    pub capabilities_per_principal: usize,
}

impl Default for GrantLimits {
    fn default() -> Self {
        Self {
            principals: 256,
            capabilities_per_principal: 64,
        }
    }
}

#[derive(Debug)]
pub struct GrantLedger {
    limits: GrantLimits,
    resources: Arc<ResourceTracker>,
    entries: RwLock<BTreeMap<Principal, BTreeMap<Capability, GrantRecord>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GrantRecord {
    decision: GrantDecision,
    sensitivity: Sensitivity,
}

impl GrantLedger {
    pub fn new(limits: GrantLimits, resources: Arc<ResourceTracker>) -> Result<Self, GrantError> {
        if limits.principals == 0 || limits.capabilities_per_principal == 0 {
            return Err(GrantError::InvalidLimits);
        }
        Ok(Self {
            limits,
            resources,
            entries: RwLock::new(BTreeMap::new()),
        })
    }

    pub fn set(
        &self,
        principal: Principal,
        capability: Capability,
        sensitivity: Sensitivity,
        decision: GrantDecision,
    ) -> Result<(), GrantError> {
        let mut entries = self.entries.write();
        if !entries.contains_key(&principal) && entries.len() >= self.limits.principals {
            return Err(GrantError::PrincipalCapacity {
                capacity: self.limits.principals,
            });
        }
        let grants = entries.entry(principal).or_default();
        if !grants.contains_key(&capability)
            && grants.len() >= self.limits.capabilities_per_principal
        {
            return Err(GrantError::CapabilityCapacity {
                capacity: self.limits.capabilities_per_principal,
            });
        }
        grants.insert(
            capability,
            GrantRecord {
                decision,
                sensitivity,
            },
        );
        Ok(())
    }

    pub fn decision(&self, principal: &Principal, capability: &Capability) -> GrantDecision {
        self.entries
            .read()
            .get(principal)
            .and_then(|grants| grants.get(capability))
            .map_or(GrantDecision::Denied, |grant| grant.decision)
    }

    pub fn revoke(
        &self,
        principal: &Principal,
        capability: &Capability,
        sessions: impl IntoIterator<Item = SessionId>,
    ) -> usize {
        if let Some(record) = self
            .entries
            .write()
            .get_mut(principal)
            .and_then(|grants| grants.get_mut(capability))
        {
            record.decision = GrantDecision::Denied;
        }

        sessions
            .into_iter()
            .map(|session| {
                self.resources
                    .cancel_session_capability(session, capability)
            })
            .sum()
    }

    /// Grants are never copied to a new aggregate hash implicitly.
    pub fn decisions_for(&self, principal: &Principal) -> Vec<(Capability, GrantDecision)> {
        self.entries
            .read()
            .get(principal)
            .into_iter()
            .flat_map(|grants| {
                grants
                    .iter()
                    .map(|(capability, record)| (capability.clone(), record.decision))
            })
            .collect()
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum GrantError {
    #[error("capability names must be finite lowercase domain identifiers")]
    InvalidCapability,
    #[error("grant limits must be finite and non-zero")]
    InvalidLimits,
    #[error("grant principal capacity {capacity} is full")]
    PrincipalCapacity { capacity: usize },
    #[error("per-principal capability capacity {capacity} is full")]
    CapabilityCapacity { capacity: usize },
}

#[cfg(test)]
mod tests {
    use crate::{ResourceClass, ResourceLimits};

    use super::*;

    fn principal(hash: char) -> Principal {
        Principal::new("a".repeat(64), "app", hash.to_string().repeat(64)).unwrap()
    }

    #[test]
    fn update_does_not_inherit_sensitive_grant() {
        let resources = Arc::new(ResourceTracker::new(ResourceLimits::default()).unwrap());
        let ledger = GrantLedger::new(GrantLimits::default(), resources).unwrap();
        let upload = Capability::new("upload").unwrap();
        ledger
            .set(
                principal('b'),
                upload.clone(),
                Sensitivity::Sensitive,
                GrantDecision::AllowExactBuild,
            )
            .unwrap();

        assert_eq!(
            ledger.decision(&principal('c'), &upload),
            GrantDecision::Denied
        );
    }

    #[test]
    fn revoke_cancels_matching_work_only() {
        let resources = Arc::new(ResourceTracker::new(ResourceLimits::default()).unwrap());
        let ledger = GrantLedger::new(GrantLimits::default(), Arc::clone(&resources)).unwrap();
        let resource = Capability::new("resource").unwrap();
        let other = Capability::new("theme").unwrap();
        let first = resources
            .admit(
                SessionId(1),
                Some(resource.clone()),
                ResourceClass::ResourceStream,
            )
            .unwrap();
        let second = resources
            .admit(SessionId(2), Some(other), ResourceClass::ProviderCall)
            .unwrap();

        assert_eq!(ledger.revoke(&principal('b'), &resource, [SessionId(1)]), 1);
        assert!(first.is_cancelled());
        assert!(!second.is_cancelled());
    }
}
