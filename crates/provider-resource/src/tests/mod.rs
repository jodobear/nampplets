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

mod blossom_svg;
mod bulk;
mod cancellation;
mod data_url;
mod errors;
mod https;
mod limits;

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
