use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Principal, ResourceTracker};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SessionId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionProfile {
    Legacy,
    Renderer,
    Hybrid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Launching,
    Running,
    Suspended,
    Crashed,
    Stopped,
}

#[derive(Debug)]
pub struct Session {
    id: SessionId,
    principal: Principal,
    profile: ExecutionProfile,
    state: Mutex<SessionState>,
    resources: Arc<ResourceTracker>,
}

impl Session {
    pub fn new(
        id: SessionId,
        principal: Principal,
        profile: ExecutionProfile,
        resources: Arc<ResourceTracker>,
    ) -> Self {
        Self {
            id,
            principal,
            profile,
            state: Mutex::new(SessionState::Launching),
            resources,
        }
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn principal(&self) -> &Principal {
        &self.principal
    }

    /// The execution profile is immutable for the entire session.
    pub fn profile(&self) -> ExecutionProfile {
        self.profile
    }

    pub fn state(&self) -> SessionState {
        *self.state.lock()
    }

    pub fn transition(&self, next: SessionState) -> Result<(), SessionError> {
        let mut state = self.state.lock();
        let valid = matches!(
            (*state, next),
            (SessionState::Launching, SessionState::Running)
                | (SessionState::Launching, SessionState::Stopped)
                | (SessionState::Running, SessionState::Suspended)
                | (SessionState::Running, SessionState::Crashed)
                | (SessionState::Running, SessionState::Stopped)
                | (SessionState::Suspended, SessionState::Running)
                | (SessionState::Suspended, SessionState::Crashed)
                | (SessionState::Suspended, SessionState::Stopped)
                | (SessionState::Crashed, SessionState::Launching)
                | (SessionState::Crashed, SessionState::Stopped)
        ) || *state == next;

        if !valid {
            return Err(SessionError::InvalidTransition {
                current: *state,
                requested: next,
            });
        }

        *state = next;
        if matches!(next, SessionState::Crashed | SessionState::Stopped) {
            self.resources.cancel_session(self.id);
        }
        Ok(())
    }

    pub fn stop(&self) {
        let mut state = self.state.lock();
        if *state != SessionState::Stopped {
            *state = SessionState::Stopped;
            self.resources.cancel_session(self.id);
        }
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            id: self.id,
            principal: self.principal.clone(),
            profile: self.profile,
            state: self.state(),
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.resources.cancel_session(self.id);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: SessionId,
    pub principal: Principal,
    pub profile: ExecutionProfile,
    pub state: SessionState,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SessionError {
    #[error("cannot transition a session from {current:?} to {requested:?}")]
    InvalidTransition {
        current: SessionState,
        requested: SessionState,
    },
}

#[cfg(test)]
mod tests {
    use crate::{ResourceLimits, ResourceTracker};

    use super::*;

    fn session() -> Session {
        Session::new(
            SessionId(1),
            Principal::new("a".repeat(64), "app", "b".repeat(64)).unwrap(),
            ExecutionProfile::Renderer,
            Arc::new(ResourceTracker::new(ResourceLimits::default()).unwrap()),
        )
    }

    #[test]
    fn profile_cannot_escalate() {
        let session = session();
        session.transition(SessionState::Running).unwrap();
        assert_eq!(session.profile(), ExecutionProfile::Renderer);
    }

    #[test]
    fn stopped_session_cannot_resume() {
        let session = session();
        session.stop();
        assert!(matches!(
            session.transition(SessionState::Running),
            Err(SessionError::InvalidTransition { .. })
        ));
    }
}
