use std::time::Duration;

use crate::{RuntimeConfig, config::MAXIMUM_CATALOG_OPERATION_DEADLINE_MILLIS};

#[test]
fn catalog_deadline_default_and_exact_maximum_are_valid() {
    assert_eq!(
        RuntimeConfig::default()
            .validated()
            .expect("default config must remain valid")
            .catalog_operation_deadline,
        Duration::from_secs(15)
    );

    let configured = RuntimeConfig {
        catalog_operation_deadline_millis: MAXIMUM_CATALOG_OPERATION_DEADLINE_MILLIS,
        ..RuntimeConfig::default()
    };
    assert!(configured.validated().is_ok());

    let configured = RuntimeConfig {
        catalog_operation_deadline_millis: 300_000,
        ..RuntimeConfig::default()
    };
    assert_eq!(
        configured
            .validated()
            .expect("a five-minute catalog deadline must remain valid")
            .catalog_operation_deadline,
        Duration::from_secs(300)
    );
}

#[test]
fn catalog_deadline_refuses_zero_and_above_maximum() {
    for milliseconds in [0, MAXIMUM_CATALOG_OPERATION_DEADLINE_MILLIS + 1] {
        let configured = RuntimeConfig {
            catalog_operation_deadline_millis: milliseconds,
            ..RuntimeConfig::default()
        };
        assert!(configured.validated().is_err());
    }
}
