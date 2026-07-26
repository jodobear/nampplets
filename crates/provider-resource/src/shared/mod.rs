//! [`crate::provider::ResourceShared`] behavior, split by concern: session
//! and quota bookkeeping in [`dispatch`], the bounded fetch/deliver flow in
//! [`fetch`].

mod dispatch;
mod fetch;
