use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    net::IpAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use nmp_native_nap_bridge::{
    BridgeLimits, DispatchOutcome, InjectionPlan, MemoryActivitySink, ProviderPushLimits,
    ProviderPushObserver, ProviderRegistry, SessionContext, SourceWindowId,
};
use nmp_native_runtime_core::{
    Cancellation, Capability, ExecutionProfile, GrantDecision, GrantLedger, GrantLimits, Principal,
    ResourceLimits, ResourceTracker, Sensitivity, SessionId,
};
use parking_lot::{Condvar, Mutex};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::*;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\npayload";
const WEBP: &[u8] = b"RIFF\x00\x00\x00\x00WEBPpayload";

#[derive(Debug, Default)]
struct FakeClock(AtomicU64);

impl ResourceClock for FakeClock {
    fn monotonic_millis(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Debug, Default)]
struct FakeActivity(Mutex<Vec<ResourceActivity>>, Condvar);

impl FakeActivity {
    fn wait_for(&self, action: ResourceActivityAction, outcome: ResourceActivityOutcome) {
        let mut facts = self.0.lock();
        while !facts
            .iter()
            .any(|fact| fact.action == action && fact.outcome == outcome)
        {
            self.1.wait(&mut facts);
        }
    }
}

impl ResourceActivitySink for FakeActivity {
    fn record(&self, fact: ResourceActivity) {
        self.0.lock().push(fact);
        self.1.notify_all();
    }
}

#[derive(Debug)]
struct FakeNetwork {
    addresses: Mutex<Vec<IpAddr>>,
    responses: Mutex<BTreeMap<String, VecDeque<Result<RawHttpsResponse, ResourceNetworkError>>>>,
    requests: Mutex<Vec<PinnedHttpsRequest>>,
    block: Mutex<bool>,
    started_count: Mutex<usize>,
    started: Condvar,
    finished: Condvar,
    finished_count: Mutex<usize>,
}

impl Default for FakeNetwork {
    fn default() -> Self {
        Self {
            addresses: Mutex::new(vec!["1.1.1.1".parse().unwrap()]),
            responses: Mutex::new(BTreeMap::new()),
            requests: Mutex::new(Vec::new()),
            block: Mutex::new(false),
            started_count: Mutex::new(0),
            started: Condvar::new(),
            finished: Condvar::new(),
            finished_count: Mutex::new(0),
        }
    }
}

impl FakeNetwork {
    fn respond(&self, url: &str, response: RawHttpsResponse) {
        self.responses
            .lock()
            .entry(url.to_owned())
            .or_default()
            .push_back(Ok(response));
    }

    fn fail(&self, url: &str, failure: ResourceNetworkError) {
        self.responses
            .lock()
            .entry(url.to_owned())
            .or_default()
            .push_back(Err(failure));
    }

    fn set_addresses(&self, addresses: &[&str]) {
        *self.addresses.lock() = addresses
            .iter()
            .map(|address| address.parse().unwrap())
            .collect();
    }

    fn set_blocking(&self) {
        *self.block.lock() = true;
    }

    fn wait_started(&self) {
        let mut started = self.started_count.lock();
        while *started == 0 {
            self.started.wait(&mut started);
        }
    }

    fn wait_finished(&self) {
        let mut finished = self.finished_count.lock();
        while *finished == 0 {
            self.finished.wait(&mut finished);
        }
    }
}

impl ResourceNetwork for FakeNetwork {
    fn resolve(
        &self,
        _request: &ResolveRequest,
        cancellation: &Cancellation,
    ) -> Result<Vec<IpAddr>, ResourceNetworkError> {
        if cancellation.is_cancelled() {
            return Err(ResourceNetworkError::Cancelled);
        }
        Ok(self.addresses.lock().clone())
    }

    fn get(
        &self,
        request: &PinnedHttpsRequest,
        cancellation: &Cancellation,
    ) -> Result<RawHttpsResponse, ResourceNetworkError> {
        self.requests.lock().push(request.clone());
        if *self.block.lock() {
            *self.started_count.lock() += 1;
            self.started.notify_all();
            cancellation.wait();
            *self.finished_count.lock() += 1;
            self.finished.notify_all();
            return Err(ResourceNetworkError::Cancelled);
        }
        self.responses
            .lock()
            .get_mut(request.url.as_ref())
            .and_then(VecDeque::pop_front)
            .unwrap_or(Err(ResourceNetworkError::NotFound))
    }
}

#[derive(Debug, Default)]
struct FakeRasterizer {
    calls: Mutex<Vec<SvgRasterRequest>>,
}

impl SvgRasterizer for FakeRasterizer {
    fn rasterize(
        &self,
        request: &SvgRasterRequest,
        cancellation: &Cancellation,
    ) -> Result<RasterizedSvg, SvgRasterError> {
        if cancellation.is_cancelled() {
            return Err(SvgRasterError::Cancelled);
        }
        self.calls.lock().push(request.clone());
        Ok(RasterizedSvg {
            bytes: WEBP.to_vec(),
            width: 32,
            height: 32,
        })
    }
}

struct Rig {
    provider: Arc<ResourceProvider>,
    network: Arc<FakeNetwork>,
    rasterizer: Arc<FakeRasterizer>,
    clock: Arc<FakeClock>,
    activity: Arc<FakeActivity>,
    registry: ProviderRegistry,
    context: SessionContext,
    plan: InjectionPlan,
    observer: ProviderPushObserver,
}

fn principal() -> Principal {
    Principal::new("a".repeat(64), "resource-test", "b".repeat(64)).unwrap()
}

fn rig_with_limits(limits: ResourceProviderLimits) -> Rig {
    let network = Arc::new(FakeNetwork::default());
    let rasterizer = Arc::new(FakeRasterizer::default());
    let clock = Arc::new(FakeClock::default());
    let activity = Arc::new(FakeActivity::default());
    let network_dyn: Arc<dyn ResourceNetwork> = network.clone();
    let rasterizer_dyn: Arc<dyn SvgRasterizer> = rasterizer.clone();
    let clock_dyn: Arc<dyn ResourceClock> = clock.clone();
    let activity_dyn: Arc<dyn ResourceActivitySink> = activity.clone();
    let provider = Arc::new(
        ResourceProvider::new(
            network_dyn,
            rasterizer_dyn,
            clock_dyn,
            activity_dyn,
            limits,
            ["https://blossom.example/files/"],
        )
        .unwrap(),
    );
    let resources = Arc::new(ResourceTracker::new(ResourceLimits::default()).unwrap());
    let grants =
        Arc::new(GrantLedger::new(GrantLimits::default(), Arc::clone(&resources)).unwrap());
    let mut registry = ProviderRegistry::new(
        BridgeLimits {
            maximum_response_bytes: 32 * 1024 * 1024,
            provider_pushes: ProviderPushLimits {
                maximum_pending_bytes: 32 * 1024 * 1024,
                maximum_envelope_bytes: 16 * 1024 * 1024,
                ..ProviderPushLimits::default()
            },
            ..BridgeLimits::default()
        },
        resources,
        Arc::clone(&grants),
        Arc::new(MemoryActivitySink::bounded(64)),
    )
    .unwrap();
    let domain = Capability::new(DOMAIN).unwrap();
    grants
        .set(
            principal(),
            domain,
            Sensitivity::Sensitive,
            GrantDecision::AllowExactBuild,
        )
        .unwrap();
    let provider_dyn: Arc<dyn Provider> = provider.clone();
    registry.register(provider_dyn).unwrap();
    let context = SessionContext {
        id: SessionId(11),
        principal: principal(),
        profile: ExecutionProfile::Legacy,
    };
    let plan = registry
        .negotiate(
            &context.principal,
            context.profile,
            &BTreeSet::from([Capability::new(DOMAIN).unwrap()]),
        )
        .unwrap();
    let observer = registry
        .open_session_bound(&context, &plan, SourceWindowId(41), 0)
        .unwrap();
    registry.mark_session_ready(context.id).unwrap();
    Rig {
        provider,
        network,
        rasterizer,
        clock,
        activity,
        registry,
        context,
        plan,
        observer,
    }
}

fn rig() -> Rig {
    rig_with_limits(ResourceProviderLimits::default())
}

fn dispatch(rig: &Rig, envelope: Value) -> DispatchOutcome {
    rig.registry
        .dispatch(
            &rig.context,
            &rig.plan,
            serde_json::to_vec(&envelope).unwrap().as_slice(),
            rig.clock.monotonic_millis(),
        )
        .unwrap()
}

async fn terminal(rig: &mut Rig, outcome: DispatchOutcome) -> Value {
    let DispatchOutcome::Handled(mut call) = outcome else {
        panic!("resource request must be handled");
    };
    let operation = call.take_operation();
    let batch = rig.observer.changed(8).await.unwrap();
    let envelope = batch.pushes[0].envelope.decode().unwrap();
    if let Some(operation) = operation {
        operation.complete();
    }
    envelope
}

#[test]
fn descriptor_and_info_are_exact_and_bounded() {
    let rig = rig();
    assert_eq!(rig.provider.descriptor().domain.as_str(), DOMAIN);
    assert_eq!(
        rig.provider.descriptor().actions,
        ["bytes", "bytesMany", "cancel", "info"]
            .into_iter()
            .map(Arc::from)
            .collect()
    );
    let DispatchOutcome::Handled(call) = dispatch(
        &rig,
        json!({
            "type": "resource.info",
            "id": "info-1",
        }),
    ) else {
        panic!("info must be handled");
    };
    assert!(!call.is_active());
    let value = call.response.unwrap().decode().unwrap();
    assert_eq!(value["type"], "resource.info.result");
    assert_eq!(value["id"], "info-1");
    assert_eq!(value["info"]["maxUrls"], 100);
    assert_eq!(
        value["info"]["schemes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["scheme"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["data", "https", "blossom"]
    );
}

#[tokio::test]
async fn data_url_is_decoded_sniffed_and_projected_as_base64_bstr() {
    let mut rig = rig();
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "data-1",
            "url": format!("data:text/html;base64,{}", STANDARD.encode(PNG)),
        }),
    );
    let value = terminal(&mut rig, outcome).await;
    assert_eq!(value["type"], "resource.bytes.result");
    assert_eq!(value["mime"], "image/png");
    assert_eq!(
        STANDARD.decode(value["blob"].as_str().unwrap()).unwrap(),
        PNG
    );
    assert!(rig.network.requests.lock().is_empty());
}

#[tokio::test]
async fn data_url_scheme_and_base64_follow_the_pinned_web_semantics() {
    let mut rig = rig();
    let bytes = [PNG, b"x"].concat();
    let encoded = STANDARD.encode(&bytes);
    let encoded = encoded.replacen("", "%0A", 1).replace('=', "%3D");
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "data-normalized",
            "url": format!("DATA:image/png;BASE64,{encoded}"),
        }),
    );
    let value = terminal(&mut rig, outcome).await;
    assert_eq!(value["type"], "resource.bytes.result");
    assert_eq!(
        STANDARD.decode(value["blob"].as_str().unwrap()).unwrap(),
        bytes
    );

    let unpadded = STANDARD.encode(&bytes).trim_end_matches('=').to_owned();
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "data-unpadded",
            "url": format!("data:image/png;base64,{unpadded}"),
        }),
    );
    assert_eq!(
        terminal(&mut rig, outcome).await["type"],
        "resource.bytes.result"
    );
    assert!(rig.network.requests.lock().is_empty());
}

#[tokio::test]
async fn https_resolution_is_pinned_and_redirect_is_rechecked() {
    let mut rig = rig();
    rig.network.respond(
        "https://images.example/a",
        RawHttpsResponse {
            status: 302,
            location: Some(Arc::from("https://cdn.example/b#redirect-fragment")),
            body: Vec::new(),
        },
    );
    rig.network.respond(
        "https://cdn.example/b",
        RawHttpsResponse {
            status: 200,
            location: None,
            body: PNG.to_vec(),
        },
    );
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "https-1",
            "url": "https://images.example/a#napplet-local-fragment",
        }),
    );
    let value = terminal(&mut rig, outcome).await;
    assert_eq!(value["type"], "resource.bytes.result");
    let requests = rig.network.requests.lock();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].url.as_ref(), "https://images.example/a");
    assert_eq!(requests[1].url.as_ref(), "https://cdn.example/b");
    assert_eq!(requests[0].host.as_ref(), "images.example");
    assert_eq!(requests[1].host.as_ref(), "cdn.example");
    assert_eq!(
        requests[0].approved_addresses.as_ref(),
        &["1.1.1.1".parse::<IpAddr>().unwrap()]
    );
}

#[tokio::test]
async fn private_or_link_local_dns_answers_fail_before_transport() {
    for address in ["127.0.0.1", "10.0.0.1", "169.254.169.254", "::1", "fc00::1"] {
        let mut rig = rig();
        rig.network.set_addresses(&[address]);
        let outcome = dispatch(
            &rig,
            json!({
                "type": "resource.bytes",
                "id": format!("blocked-{address}"),
                "url": "https://images.example/a",
            }),
        );
        let value = terminal(&mut rig, outcome).await;
        assert_eq!(value["error"], "blocked-by-policy");
        assert!(rig.network.requests.lock().is_empty());
    }
}

#[tokio::test]
async fn oversized_dns_answer_set_is_refused_before_transport() {
    let limits = ResourceProviderLimits {
        maximum_resolved_addresses: 1,
        ..ResourceProviderLimits::default()
    };
    let mut rig = rig_with_limits(limits);
    rig.network.set_addresses(&["1.1.1.1", "8.8.8.8"]);
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "dns-cap",
            "url": "https://images.example/a",
        }),
    );
    assert_eq!(terminal(&mut rig, outcome).await["error"], "too-large");
    assert!(rig.network.requests.lock().is_empty());
}

#[tokio::test]
async fn http_redirect_and_credentialed_url_are_refused() {
    let mut rig = rig();
    rig.network.respond(
        "https://images.example/a",
        RawHttpsResponse {
            status: 302,
            location: Some(Arc::from("http://127.0.0.1/admin")),
            body: Vec::new(),
        },
    );
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "redirect-1",
            "url": "https://images.example/a",
        }),
    );
    assert_eq!(
        terminal(&mut rig, outcome).await["error"],
        "blocked-by-policy"
    );

    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "credential-1",
            "url": "https://user:secret@images.example/a",
        }),
    );
    assert_eq!(
        terminal(&mut rig, outcome).await["error"],
        "blocked-by-policy"
    );
}

#[tokio::test]
async fn redirect_limit_and_missing_location_are_typed_failures() {
    let limits = ResourceProviderLimits {
        maximum_redirects: 1,
        ..ResourceProviderLimits::default()
    };
    let mut limited_rig = rig_with_limits(limits);
    limited_rig.network.respond(
        "https://images.example/a",
        RawHttpsResponse {
            status: 302,
            location: Some(Arc::from("https://images.example/b")),
            body: Vec::new(),
        },
    );
    limited_rig.network.respond(
        "https://images.example/b",
        RawHttpsResponse {
            status: 302,
            location: Some(Arc::from("https://images.example/c")),
            body: Vec::new(),
        },
    );
    let outcome = dispatch(
        &limited_rig,
        json!({
            "type": "resource.bytes",
            "id": "redirect-cap",
            "url": "https://images.example/a",
        }),
    );
    assert_eq!(
        terminal(&mut limited_rig, outcome).await["error"],
        "blocked-by-policy"
    );
    assert_eq!(limited_rig.network.requests.lock().len(), 2);

    let mut rig = rig();
    rig.network.respond(
        "https://images.example/missing-location",
        RawHttpsResponse {
            status: 302,
            location: None,
            body: Vec::new(),
        },
    );
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "missing-location",
            "url": "https://images.example/missing-location",
        }),
    );
    assert_eq!(terminal(&mut rig, outcome).await["error"], "network-error");
}

#[tokio::test]
async fn oversize_and_mime_confusion_are_typed_failures() {
    let limits = ResourceProviderLimits {
        maximum_response_bytes: 64,
        maximum_svg_bytes: 64,
        maximum_bulk_response_bytes: 128,
        maximum_blob_bytes_per_request: 128,
        ..ResourceProviderLimits::default()
    };
    let mut rig = rig_with_limits(limits);
    rig.network.respond(
        "https://images.example/large",
        RawHttpsResponse {
            status: 200,
            location: None,
            body: vec![0_u8; 65],
        },
    );
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "large",
            "url": "https://images.example/large",
        }),
    );
    assert_eq!(terminal(&mut rig, outcome).await["error"], "too-large");

    rig.network.respond(
        "https://images.example/html",
        RawHttpsResponse {
            status: 200,
            location: None,
            body: b"<html>attacker-labelled image/png</html>".to_vec(),
        },
    );
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "mime",
            "url": "https://images.example/html",
        }),
    );
    assert_eq!(terminal(&mut rig, outcome).await["error"], "decode-failed");
}

#[tokio::test]
async fn network_timeout_remains_distinct_from_other_network_failure() {
    let mut rig = rig();
    rig.network
        .fail("https://images.example/slow", ResourceNetworkError::Timeout);
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "timeout",
            "url": "https://images.example/slow",
        }),
    );
    assert_eq!(terminal(&mut rig, outcome).await["error"], "timeout");
}

#[tokio::test]
async fn raw_svg_is_only_delivered_after_bounded_no_network_rasterization() {
    let mut rig = rig();
    let svg = b"<?xml version=\"1.0\"?><svg><script>alert(1)</script></svg>";
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "svg",
            "url": format!("data:image/png;base64,{}", STANDARD.encode(svg)),
        }),
    );
    let value = terminal(&mut rig, outcome).await;
    assert_eq!(value["mime"], "image/webp");
    assert_eq!(
        STANDARD.decode(value["blob"].as_str().unwrap()).unwrap(),
        WEBP
    );
    let calls = rig.rasterizer.calls.lock();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].source.as_ref(), svg);
    assert_eq!(calls[0].maximum_dimension, 4_096);
}

#[tokio::test]
async fn blossom_hash_is_verified_before_mime_delivery() {
    let mut rig = rig();
    let digest = hex::encode(Sha256::digest(PNG));
    rig.network.respond(
        &format!("https://blossom.example/files/{digest}"),
        RawHttpsResponse {
            status: 200,
            location: None,
            body: PNG.to_vec(),
        },
    );
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "blossom-ok",
            "url": format!("blossom:sha256:{digest}"),
        }),
    );
    assert_eq!(
        terminal(&mut rig, outcome).await["type"],
        "resource.bytes.result"
    );

    let wrong = "0".repeat(64);
    rig.network.respond(
        &format!("https://blossom.example/files/{wrong}"),
        RawHttpsResponse {
            status: 200,
            location: None,
            body: PNG.to_vec(),
        },
    );
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "blossom-bad",
            "url": format!("blossom:sha256:{wrong}"),
        }),
    );
    assert_eq!(terminal(&mut rig, outcome).await["error"], "decode-failed");
}

#[tokio::test]
async fn bulk_preserves_order_and_per_item_failure() {
    let mut rig = rig();
    let first = format!("data:image/png;base64,{}", STANDARD.encode(PNG));
    let third = format!("data:image/webp;base64,{}", STANDARD.encode(WEBP));
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytesMany",
            "id": "bulk",
            "urls": [first, "http://blocked.example/a", third],
        }),
    );
    let value = terminal(&mut rig, outcome).await;
    let items = value["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["ok"], true);
    assert_eq!(items[1]["ok"], false);
    assert_eq!(items[1]["error"], "unsupported-scheme");
    assert_eq!(items[2]["ok"], true);
}

#[tokio::test]
async fn bulk_enforces_its_byte_ceiling_per_item_without_discarding_later_siblings() {
    let limits = ResourceProviderLimits {
        maximum_response_bytes: 64,
        maximum_svg_bytes: 64,
        maximum_bulk_response_bytes: 24,
        maximum_blob_bytes_per_request: 128,
        ..ResourceProviderLimits::default()
    };
    let mut rig = rig_with_limits(limits);
    let full = format!("data:image/png;base64,{}", STANDARD.encode(PNG));
    let small = format!(
        "data:image/png;base64,{}",
        STANDARD.encode(b"\x89PNG\r\n\x1a\n")
    );
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytesMany",
            "id": "bulk-bytes",
            "urls": [full.clone(), full, small],
        }),
    );
    let value = terminal(&mut rig, outcome).await;
    let items = value["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["ok"], true);
    assert_eq!(items[1]["ok"], false);
    assert_eq!(items[1]["error"], "quota-exceeded");
    assert_eq!(items[2]["ok"], true);
}

#[test]
fn empty_or_over_cap_bulk_gets_one_top_level_error() {
    let rig = rig();
    let DispatchOutcome::Handled(call) = dispatch(
        &rig,
        json!({
            "type": "resource.bytesMany",
            "id": "empty",
            "urls": [],
        }),
    ) else {
        panic!("request must be handled");
    };
    assert!(!call.is_active());
    assert_eq!(
        call.response.unwrap().decode().unwrap()["error"],
        "invalid-request"
    );

    let urls = (0..=ResourceProviderLimits::default().maximum_urls_per_bulk)
        .map(|index| format!("https://images.example/{index}"))
        .collect::<Vec<_>>();
    let DispatchOutcome::Handled(call) = dispatch(
        &rig,
        json!({
            "type": "resource.bytesMany",
            "id": "over-cap",
            "urls": urls,
        }),
    ) else {
        panic!("request must be handled");
    };
    assert!(!call.is_active());
    let value = call.response.unwrap().decode().unwrap();
    assert_eq!(value["type"], "resource.bytesMany.error");
    assert_eq!(value["error"], "too-large");
}

#[test]
fn correlated_malformed_requests_get_typed_action_error_terminals() {
    let rig = rig();
    for (request, expected_type) in [
        (
            json!({
                "type": "resource.info",
                "id": "bad-info",
                "unexpected": true,
            }),
            "resource.info.error",
        ),
        (
            json!({
                "type": "resource.bytes",
                "id": "bad-bytes",
                "url": 42,
            }),
            "resource.bytes.error",
        ),
        (
            json!({
                "type": "resource.bytesMany",
                "id": "bad-many-shape",
                "urls": "not-an-array",
            }),
            "resource.bytesMany.error",
        ),
        (
            json!({
                "type": "resource.bytesMany",
                "id": "bad-many-item",
                "urls": ["https://images.example/a", 42],
            }),
            "resource.bytesMany.error",
        ),
    ] {
        let DispatchOutcome::Handled(call) = dispatch(&rig, request) else {
            panic!("well-correlated malformed request must be handled");
        };
        assert!(!call.is_active());
        let value = call.response.unwrap().decode().unwrap();
        assert_eq!(value["type"], expected_type);
        assert_eq!(value["error"], "invalid-request");
    }
}

#[test]
fn provider_rejects_invalid_configuration_before_advertisement() {
    let network: Arc<dyn ResourceNetwork> = Arc::new(FakeNetwork::default());
    let rasterizer: Arc<dyn SvgRasterizer> = Arc::new(FakeRasterizer::default());
    let clock: Arc<dyn ResourceClock> = Arc::new(FakeClock::default());
    let activity: Arc<dyn ResourceActivitySink> = Arc::new(FakeActivity::default());
    assert!(matches!(
        ResourceProvider::new(
            Arc::clone(&network),
            Arc::clone(&rasterizer),
            Arc::clone(&clock),
            Arc::clone(&activity),
            ResourceProviderLimits::default(),
            std::iter::empty::<Arc<str>>(),
        ),
        Err(ResourceProviderBuildError::MissingBlossomServer)
    ));
    assert!(matches!(
        ResourceProvider::new(
            network,
            rasterizer,
            clock,
            activity,
            ResourceProviderLimits::default(),
            ["http://localhost/"],
        ),
        Err(ResourceProviderBuildError::InvalidBlossomServer { .. })
    ));
}

#[test]
fn crash_or_revoke_cancels_blocking_fetch_and_drops_late_terminal() {
    let rig = rig();
    rig.network.set_blocking();
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "slow",
            "url": "https://images.example/slow",
        }),
    );
    let DispatchOutcome::Handled(call) = outcome else {
        panic!("request must be handled");
    };
    assert!(call.is_active());
    rig.network.wait_started();
    rig.registry.close_session_with_reason(
        rig.context.id,
        nmp_native_nap_bridge::ProviderSessionEnd::Crashed,
    );
    rig.network.wait_finished();
    assert!(call.operation().unwrap().is_cancelled());
    assert_eq!(rig.provider.census().active_requests, 0);
    assert!(rig.observer.drain(8).unwrap().pushes.is_empty());
}

#[test]
fn explicit_cancel_is_idempotent_and_activity_facts_omit_urls() {
    let rig = rig();
    rig.network.set_blocking();
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "cancel-me",
            "url": "https://secret-interest.example/avatar",
        }),
    );
    let DispatchOutcome::Handled(call) = outcome else {
        panic!("request must be handled");
    };
    rig.network.wait_started();
    let DispatchOutcome::Handled(cancel) = dispatch(
        &rig,
        json!({
            "type": "resource.cancel",
            "id": "cancel-me",
        }),
    ) else {
        panic!("cancel must be handled");
    };
    assert!(!cancel.is_active());
    rig.network.wait_finished();
    rig.activity.wait_for(
        ResourceActivityAction::Bytes,
        ResourceActivityOutcome::Cancelled,
    );
    assert!(call.operation().unwrap().is_cancelled());
    assert!(rig.observer.drain(8).unwrap().pushes.is_empty());
    let facts = rig.activity.0.lock();
    assert!(facts.iter().any(|fact| {
        fact.action == ResourceActivityAction::Cancel
            && fact.outcome == ResourceActivityOutcome::Cancelled
    }));
}

#[test]
fn bulk_cancel_drops_partial_results_and_releases_the_full_reservation() {
    let rig = rig();
    rig.network.set_blocking();
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytesMany",
            "id": "cancel-bulk",
            "urls": [
                "https://images.example/one",
                "https://images.example/two",
            ],
        }),
    );
    let DispatchOutcome::Handled(call) = outcome else {
        panic!("request must be handled");
    };
    rig.network.wait_started();
    let DispatchOutcome::Handled(cancel) = dispatch(
        &rig,
        json!({
            "type": "resource.cancel",
            "id": "cancel-bulk",
        }),
    ) else {
        panic!("cancel must be handled");
    };
    assert!(!cancel.is_active());
    rig.network.wait_finished();
    rig.activity.wait_for(
        ResourceActivityAction::BytesMany,
        ResourceActivityOutcome::Cancelled,
    );
    assert!(call.operation().unwrap().is_cancelled());
    assert_eq!(rig.provider.census().active_requests, 0);
    assert_eq!(rig.provider.census().in_flight_urls, 0);
    assert!(rig.observer.drain(8).unwrap().pushes.is_empty());
}

#[test]
fn per_napplet_concurrency_and_rate_refuse_without_queueing() {
    let limits = ResourceProviderLimits {
        maximum_requests_per_napplet_per_minute: 1,
        maximum_in_flight_urls_per_napplet: 1,
        ..ResourceProviderLimits::default()
    };
    let rig = rig_with_limits(limits);
    rig.network.set_blocking();
    let first = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "first",
            "url": "https://images.example/first",
        }),
    );
    let DispatchOutcome::Handled(first) = first else {
        panic!("request must be handled");
    };
    rig.network.wait_started();
    let DispatchOutcome::Handled(second) = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "second",
            "url": "https://images.example/second",
        }),
    ) else {
        panic!("request must be handled");
    };
    assert!(!second.is_active());
    assert_eq!(
        second.response.unwrap().decode().unwrap()["error"],
        "blocked-by-policy"
    );
    rig.registry.close_session(rig.context.id);
    rig.network.wait_finished();
    assert!(first.operation().unwrap().is_cancelled());
}
