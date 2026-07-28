//! Exact `@napplet/nap` 0.29.0 identity-provider contract.
//!
//! The provider is read-only. It never receives signer capabilities or secret
//! key material. The data-plane port freezes the active public account before
//! each query and must scope every NMP read to that exact account.

pub use nmp_native_runtime_core::{
    PublicIdentity as FrozenIdentity, PublicIdentityChangeSink as IdentityChangeListener,
    PublicIdentityDataPlane as IdentityDataPlane, PublicIdentityError as IdentityDataError,
    PublicIdentityObservation as AccountObservationHandle, PublicIdentityQuery as IdentityQuery,
    PublicIdentityRead as IdentityRead, PublicIdentityReadLimits as IdentityReadLimits,
    PublicIdentitySubscription as AccountObservation,
};

pub const DOMAIN: &str = "identity";
pub const PINNED_NAP_PROTOCOL: &str = "napplet-web@0.29.0";
pub const PINNED_NPM_TARBALL_SHA256: &str =
    "5e3e086bbb83335efb1d35c68cc0cd88780ab60e2bc7dbb3bd9daac88f72909f";

mod diagnostics;
mod provider;
mod types;
mod validate;
mod wire;

pub use diagnostics::*;
pub use provider::*;
pub use types::*;

#[cfg(test)]
mod tests;
