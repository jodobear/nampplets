use super::*;

use std::sync::mpsc::{self, SyncSender};

use nmp_native_nap_bridge::{
    ProviderPushLimits, ProviderPushSender, ProviderSession, ProviderSessionContext,
};

const LARGE_RESPONSE_BYTES: usize = 600 * 1_024;

#[derive(Debug)]
struct StreamingResourceProvider {
    descriptor: ProviderDescriptor,
    outbound: Mutex<BTreeMap<SessionId, ProviderPushSender>>,
}

struct ProviderPushRecorder(SyncSender<String>);

impl RuntimeObserver for ProviderPushRecorder {
    fn update(&self, frame: RuntimeObservationFrame) {
        let response = frame.events.into_iter().find_map(|event| {
            let response = event.response_json?;
            (serde_json::from_str::<Value>(&response).ok()?.get("type")? == "resource.bytes.result")
                .then_some(response)
        });
        if let Some(response) = response {
            let _ = self.0.try_send(response);
        }
    }
}

impl StreamingResourceProvider {
    fn new() -> Self {
        Self {
            descriptor: ProviderDescriptor {
                domain: Capability::new("resource").unwrap(),
                protocol_versions: BTreeSet::from([Arc::from(PINNED_NAP_PROTOCOL)]),
                actions: BTreeSet::from([Arc::from("bytes")]),
                sensitive: true,
                dependencies: BTreeSet::new(),
                platform_availability: ProviderPlatformAvailability::Available,
            },
            outbound: Mutex::new(BTreeMap::new()),
        }
    }
}

impl Provider for StreamingResourceProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn call(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        let sender = self
            .outbound
            .lock()
            .get(&request.session)
            .cloned()
            .ok_or_else(|| ProviderError::Failed {
                domain: Arc::from("resource"),
                action: Arc::clone(&request.action),
                reason: Arc::from("missing exact-session outbound lane"),
            })?;
        let id = request
            .correlation_id
            .as_deref()
            .unwrap_or("large-response")
            .to_owned();
        sender
            .push(
                "resource.bytes.result",
                serde_json::Map::from_iter([
                    ("id".to_owned(), Value::String(id)),
                    (
                        "bytesBase64".to_owned(),
                        Value::String("A".repeat(LARGE_RESPONSE_BYTES)),
                    ),
                ]),
                None,
            )
            .map_err(|error| ProviderError::Failed {
                domain: Arc::from("resource"),
                action: Arc::clone(&request.action),
                reason: Arc::from(error.to_string()),
            })?;
        Ok(ProviderCall::streaming(None, request.work))
    }

    fn session_opened(&self, session: ProviderSession) -> Result<(), ProviderError> {
        self.outbound
            .lock()
            .insert(session.context.session, session.outbound);
        Ok(())
    }

    fn session_closed(
        &self,
        session: &ProviderSessionContext,
        _reason: nmp_native_nap_bridge::ProviderSessionEnd,
    ) {
        self.outbound.lock().remove(&session.session);
    }
}

#[test]
fn provider_push_config_defaults_match_bridge_defaults() {
    let config = RuntimeConfig::default();
    let defaults = ProviderPushLimits::default();
    assert_eq!(
        config.maximum_provider_push_envelope_bytes,
        defaults.maximum_envelope_bytes as u64
    );
    assert_eq!(
        config.maximum_provider_push_pending_bytes,
        defaults.maximum_pending_bytes as u64
    );

    let validated = config.validated().unwrap();
    assert_eq!(validated.provider_push_limits, defaults);
}

#[test]
fn provider_push_config_rejects_zero_overflow_and_inconsistent_pairs() {
    for config in [
        RuntimeConfig {
            maximum_provider_push_envelope_bytes: 0,
            ..RuntimeConfig::default()
        },
        RuntimeConfig {
            maximum_provider_push_pending_bytes: 0,
            ..RuntimeConfig::default()
        },
    ] {
        assert_open_rejects(config);
    }

    assert_open_rejects(RuntimeConfig {
        maximum_provider_push_pending_bytes: u64::MAX,
        ..RuntimeConfig::default()
    });

    assert_open_rejects(RuntimeConfig {
        maximum_provider_push_envelope_bytes: 1_024,
        maximum_provider_push_pending_bytes: 1_023,
        ..RuntimeConfig::default()
    });
}

fn assert_open_rejects(config: RuntimeConfig) {
    assert!(matches!(
        RuntimeController::open(config, Box::new(FixtureSource(BTreeMap::new()))),
        Err(RuntimeOpenError::InvalidConfig { .. })
    ));
}

#[test]
fn opted_in_host_receives_streaming_response_larger_than_default_envelope() {
    let temp = TempDir::new().unwrap();
    let provider = Arc::new(StreamingResourceProvider::new());
    let controller = RuntimeController::open_with_rust_providers(
        RuntimeConfig {
            runtime_store_path: temp.path().join("runtime.sqlite3").display().to_string(),
            nmp_store_path: None,
            artifact_cache_path: temp.path().join("artifacts").display().to_string(),
            maximum_provider_push_envelope_bytes: 1_024 * 1_024,
            maximum_provider_push_pending_bytes: 2 * 1_024 * 1_024,
            ..RuntimeConfig::default()
        },
        Box::new(FixtureSource(BTreeMap::from([(
            DIGEST.to_owned(),
            INDEX.to_vec(),
        )]))),
        vec![provider],
    )
    .unwrap();

    let (_, session) = install_and_launch(&controller, &[]);
    let (send, receive) = mpsc::sync_channel(1);
    let observation = controller
        .clone()
        .observe(Box::new(ProviderPushRecorder(send)))
        .observation
        .expect("provider-push observer admitted");
    controller.mapped_envelope(
        session,
        br#"{"type":"resource.bytes","id":"large-response","url":"https://example.test/picture.jpg"}"#
            .to_vec(),
    );

    let response = receive.recv().expect("large provider push delivered");
    observation.stop();
    let envelope = BoundedJson::from_raw(&response, 1_024 * 1_024).unwrap();

    assert!(
        envelope.byte_len() > ProviderPushLimits::default().maximum_envelope_bytes,
        "test response must exceed the backward-compatible default"
    );
    assert!(
        envelope.byte_len() > ProviderPushLimits::default().maximum_pending_bytes,
        "test response must require an opted-in aggregate pending-byte bound"
    );
    assert_eq!(
        envelope.decode().unwrap()["id"],
        Value::String("large-response".to_owned())
    );
    controller.close();
}
