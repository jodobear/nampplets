import NMPNativeRuntimeApple
import Testing
@testable import RuntimeWorkbenchFeature

private func nativeSnapshot(
    revision: UInt64 = 1,
    observing: Bool = true,
    relays: [NativeRuntimeRelayDiagnostics] = [],
    omittedRelays: UInt64 = 0
) -> NativeRuntimeRelayDiagnosticsSnapshot {
    NativeRuntimeRelayDiagnosticsSnapshot(
        revision: revision,
        observing: observing,
        relays: relays,
        omittedRelays: omittedRelays,
        uncoveredAuthorCount: 0,
        droppedMergeRules: [],
        omittedDroppedMergeRules: 0,
        discoveredPrivateRelaysRejected: 0,
        sessionsRejectedOverCap: 0,
        storeDegraded: nil,
        transportDegraded: nil,
        failure: nil
    )
}

private func nativeRelay(
    relay: String = "wss://relay.example",
    subscriptions: [NativeRuntimeRelaySubscription] = [],
    omittedSubscriptions: UInt64 = 0
) -> NativeRuntimeRelayDiagnostics {
    NativeRuntimeRelayDiagnostics(
        relay: relay,
        access: .public,
        wireSubscriptionCount: UInt64(subscriptions.count),
        authorsServed: 0,
        lanes: [],
        omittedLanes: 0,
        subscriptions: subscriptions,
        omittedSubscriptions: omittedSubscriptions,
        eventsByKind: [],
        omittedKinds: 0,
        supportedNips: nil,
        omittedSupportedNips: 0,
        nip11DocumentRevision: nil,
        nip11Freshness: nil,
        nip11LastError: nil,
        nip77Advertisement: "unknown",
        nip77Behavior: "unknown",
        nip77Handoff: "unknown"
    )
}

@Suite("Relay diagnostics models")
struct RelayDiagnosticsModelsTests {
    @Test("notObserving default reports empty relays as not currently accounted")
    func notObservingDefaultIsExplicit() {
        let snapshot = InspectorRelayDiagnosticsSnapshot.notObserving
        #expect(!snapshot.observing)
        #expect(snapshot.relays.isEmpty)
    }

    @Test("observing flag is preserved through the projection")
    func observingFlagRoundTrips() {
        let observingTrue = InspectorRelayDiagnosticsSnapshot(nativeSnapshot(observing: true))
        #expect(observingTrue.observing)

        let observingFalse = InspectorRelayDiagnosticsSnapshot(nativeSnapshot(observing: false))
        #expect(!observingFalse.observing)
    }

    @Test("oversized subscription lists are truncated and folded into omittedSubscriptions")
    func subscriptionOverflowIsCountedNotDropped() throws {
        let subscriptions = (0..<(InspectorRelayLimits.maximumSubscriptionsPerRelay + 5)).map {
            NativeRuntimeRelaySubscription(filter: "{\"kinds\":[\($0)]}", coverage: nil)
        }
        let relay = nativeRelay(subscriptions: subscriptions, omittedSubscriptions: 2)
        let snapshot = InspectorRelayDiagnosticsSnapshot(nativeSnapshot(relays: [relay]))

        let projected = try #require(snapshot.relays.first)
        #expect(projected.subscriptions.count == InspectorRelayLimits.maximumSubscriptionsPerRelay)
        // 5 truncated locally, plus the 2 the runtime already reported omitted.
        #expect(projected.omittedSubscriptions == 7)
    }

    @Test("a relay with an empty URL is dropped and folded into omittedRelays")
    func malformedRelayIsOmittedNotFabricated() {
        let malformed = nativeRelay(relay: "")
        let snapshot = InspectorRelayDiagnosticsSnapshot(
            nativeSnapshot(relays: [malformed], omittedRelays: 1)
        )

        #expect(snapshot.relays.isEmpty)
        #expect(snapshot.omittedRelays == 2)
    }

    @Test("nil coverage is preserved as unproven, never coerced to zero")
    func unprovenCoverageStaysNil() throws {
        let subscription = NativeRuntimeRelaySubscription(filter: "{\"kinds\":[1]}", coverage: nil)
        let relay = nativeRelay(subscriptions: [subscription])
        let snapshot = InspectorRelayDiagnosticsSnapshot(nativeSnapshot(relays: [relay]))

        let projected = try #require(snapshot.relays.first?.subscriptions.first)
        #expect(projected.coverage == nil)
    }
}

@MainActor
private final class FakeRelayDiagnosticsSubscription: RelayDiagnosticsSubscription {
    private(set) var isCancelled = false

    func cancel() {
        isCancelled = true
    }
}

@MainActor
private final class FakeRelayDiagnosticsSource: RelayDiagnosticsSource {
    private(set) var subscribeCallCount = 0
    private(set) var subscription = FakeRelayDiagnosticsSubscription()
    private var receiver: (@MainActor (InspectorRelayDiagnosticsSnapshot) -> Void)?

    func subscribe(
        receive: @escaping @MainActor (InspectorRelayDiagnosticsSnapshot) -> Void
    ) -> any RelayDiagnosticsSubscription {
        subscribeCallCount += 1
        receiver = receive
        receive(InspectorRelayDiagnosticsSnapshot(nativeSnapshot(relays: [nativeRelay()])))
        return subscription
    }

    func push(_ snapshot: InspectorRelayDiagnosticsSnapshot) {
        receiver?(snapshot)
    }
}

@Suite("Relay diagnostics view model")
@MainActor
struct RelayDiagnosticsViewModelTests {
    @Test("start subscribes exactly once and stop withdraws the real observation")
    func startStopLifecycleIsExplicit() {
        let source = FakeRelayDiagnosticsSource()
        let model = RelayDiagnosticsViewModel(source: source)

        model.start()
        #expect(source.subscribeCallCount == 1)
        #expect(model.snapshot.relays.count == 1)

        model.start()
        #expect(source.subscribeCallCount == 1, "a second start must not open a second observation")

        model.stop()
        #expect(source.subscription.isCancelled)
        #expect(!model.snapshot.observing, "stopping must not leave a stale observing snapshot")
    }

    @Test("later updates replace the snapshot while observing")
    func laterUpdatesReplaceSnapshot() {
        let source = FakeRelayDiagnosticsSource()
        let model = RelayDiagnosticsViewModel(source: source)
        model.start()

        let next = InspectorRelayDiagnosticsSnapshot(
            nativeSnapshot(revision: 2, relays: [nativeRelay(relay: "wss://relay.two")])
        )
        source.push(next)

        #expect(model.snapshot.revision == 2)
        #expect(model.snapshot.relays.first?.relay == "wss://relay.two")
    }
}
