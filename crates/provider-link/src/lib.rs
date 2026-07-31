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

pub const PINNED_NAP_PROTOCOL: &str = "napplet-web@0.29.0";

/// Last intent protocol whose advertised carrier this runtime implements.
///
/// The 0.29 package contract requires independent `intent.deliver` target
/// delivery. Runtime dispatch still uses the older INC subscription carrier,
/// so the intent descriptor must not claim 0.29 support yet.
pub const SUPPORTED_INTENT_PROTOCOL: &str = "napplet-web@0.28.0";

#[cfg(test)]
mod protocol_tests {
    use std::sync::Arc;

    use nmp_native_nap_bridge::Provider;

    use super::*;

    #[derive(Debug)]
    struct UnavailableDispatcher;

    impl NativeIntentDispatcher for UnavailableDispatcher {
        fn try_dispatch(
            &self,
            _request: NativeIntentDispatch,
        ) -> Result<Arc<str>, NativeIntentStartError> {
            Err(NativeIntentStartError::Unavailable)
        }

        fn cancel(&self, _native_handle: &str) {}
    }

    #[test]
    fn intent_descriptor_stays_on_prior_protocol_until_independent_delivery_exists() {
        let provider = IntentProvider::new(
            Arc::new(ConfirmEveryIntent),
            Arc::new(CancelIntentChoice),
            Arc::new(UnavailableDispatcher),
            Arc::new(NoopIntentActivity),
            IntentProviderLimits::default(),
        )
        .unwrap();

        assert_eq!(
            provider.descriptor().protocol_versions,
            [Arc::from(SUPPORTED_INTENT_PROTOCOL)].into_iter().collect()
        );
        assert!(
            !provider
                .descriptor()
                .protocol_versions
                .contains(PINNED_NAP_PROTOCOL)
        );
    }
}
