use std::{collections::BTreeSet, sync::Arc};

use nmp_native_nap_bridge::{
    Provider, ProviderCall, ProviderDescriptor, ProviderError, ProviderPlatformAvailability,
    ProviderRequest,
};
use nmp_native_runtime_core::Capability;

#[derive(Debug)]
struct DependencyProvider {
    descriptor: ProviderDescriptor,
}

impl Provider for DependencyProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn call(&self, _request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        Ok(ProviderCall::completed(None))
    }
}

pub(super) fn dependency_provider(domain: Capability) -> Arc<dyn Provider> {
    Arc::new(DependencyProvider {
        descriptor: ProviderDescriptor {
            domain,
            protocol_versions: BTreeSet::from([Arc::from("test")]),
            actions: BTreeSet::from([Arc::from("noop")]),
            sensitive: true,
            dependencies: BTreeSet::new(),
            platform_availability: ProviderPlatformAvailability::Available,
        },
    })
}
