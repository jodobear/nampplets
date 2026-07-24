//! Bounded NAP-LINK and NAP-INTENT policy kernels.
//!
//! Rust validates and resolves every request. Injected native executors only
//! present confirmation/choice UI and execute the already-authorized OS
//! capability. They receive cancellation signals and opaque operation tokens;
//! they never infer caller identity or routing policy from component data.

mod intent;
mod link;

pub use intent::*;
pub use link::*;

pub const PINNED_NAP_PROTOCOL: &str = "napplet-web@0.28.0";
