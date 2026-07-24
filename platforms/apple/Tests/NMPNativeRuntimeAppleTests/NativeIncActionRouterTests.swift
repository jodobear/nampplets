import Foundation
import NMPNativeRuntime
import XCTest
@testable import NMPNativeRuntimeApple

final class NativeIncActionRouterTests: XCTestCase {
    func testMissingHandlerRefusesAsClosed() {
        let executor = MacOSIncActionExecutor()

        XCTAssertEqual(
            executor.tryEnqueue(request: Self.request()),
            .closed
        )
    }

    func testPendingCapacityRefusesWithBackpressure() {
        let executor = MacOSIncActionExecutor()
        let results = LockedValue<[NativeIncActionEnqueueResult]>([])
        let admitted = expectation(description: "all capacity attempts completed")
        let delivered = expectation(description: "admitted actions delivered")
        delivered.expectedFulfillmentCount = 64
        executor.setHandler { _ in delivered.fulfill() }

        DispatchQueue.main.async {
            let attempts = (0 ... 64).map { index in
                executor.tryEnqueue(
                    request: Self.request(
                        payloadJSON: #"{"sequence":\#(index)}"#
                    )
                )
            }
            results.set(attempts)
            admitted.fulfill()
        }

        wait(for: [admitted, delivered], timeout: 2)
        XCTAssertEqual(
            Array(results.value.prefix(64)),
            Array(repeating: .accepted, count: 64)
        )
        XCTAssertEqual(results.value.last, .backpressure)
    }

    func testDeliveryIsFIFOOnMainQueue() {
        let executor = MacOSIncActionExecutor()
        let received = LockedValue<[String]>([])
        let delivered = expectation(description: "FIFO actions delivered")
        delivered.expectedFulfillmentCount = 4
        let admitted = expectation(description: "FIFO actions admitted")
        let payloads = (0 ..< 4).map { #"{"sequence":\#($0)}"# }
        executor.setHandler { action in
            XCTAssertTrue(Thread.isMainThread)
            received.withValue { $0.append(action.payloadJSON) }
            delivered.fulfill()
        }

        DispatchQueue.main.async {
            for payload in payloads {
                XCTAssertEqual(
                    executor.tryEnqueue(
                        request: Self.request(payloadJSON: payload)
                    ),
                    .accepted
                )
            }
            admitted.fulfill()
        }

        wait(for: [admitted, delivered], timeout: 2)
        XCTAssertEqual(received.value, payloads)
    }

    func testSessionEndedPurgesOnlyTheExactSessionAndWindow() {
        let executor = MacOSIncActionExecutor()
        let received = LockedValue<[NativeWorkbenchAction]>([])
        let delivered = expectation(description: "nonmatching actions delivered")
        delivered.expectedFulfillmentCount = 2
        let drained = expectation(description: "main action queue drained")
        executor.setHandler { action in
            received.withValue { $0.append(action) }
            delivered.fulfill()
        }

        DispatchQueue.main.async {
            let requests = [
                Self.request(
                    sessionID: 7,
                    sourceWindowID: 70,
                    payloadJSON: #"{"name":"purged-one"}"#
                ),
                Self.request(
                    sessionID: 7,
                    sourceWindowID: 70,
                    payloadJSON: #"{"name":"purged-two"}"#
                ),
                Self.request(
                    sessionID: 7,
                    sourceWindowID: 71,
                    payloadJSON: #"{"name":"same-session"}"#
                ),
                Self.request(
                    sessionID: 8,
                    sourceWindowID: 70,
                    payloadJSON: #"{"name":"same-window"}"#
                ),
            ]
            for request in requests {
                XCTAssertEqual(
                    executor.tryEnqueue(request: request),
                    .accepted
                )
            }
            executor.sessionEnded(
                end: Self.end(sessionID: 7, sourceWindowID: 70)
            )
            DispatchQueue.main.async { drained.fulfill() }
        }

        wait(for: [delivered, drained], timeout: 2)
        XCTAssertEqual(received.value.map(\.sessionID), [7, 8])
        XCTAssertEqual(received.value.map(\.sourceWindowID), [71, 70])
        XCTAssertEqual(
            received.value.map(\.payloadJSON),
            [
                #"{"name":"same-session"}"#,
                #"{"name":"same-window"}"#,
            ]
        )
    }

    func testRemovingHandlerPurgesQueuedActions() {
        let executor = MacOSIncActionExecutor()
        let received = LockedValue<[String]>([])
        let delivered = expectation(description: "replacement handler delivered")
        let drained = expectation(description: "main action queue drained")
        executor.setHandler { action in
            received.withValue { $0.append("stale:\(action.payloadJSON)") }
        }

        DispatchQueue.main.async {
            XCTAssertEqual(
                executor.tryEnqueue(
                    request: Self.request(payloadJSON: #"{"value":"stale"}"#)
                ),
                .accepted
            )
            executor.setHandler(nil)
            executor.setHandler { action in
                received.withValue {
                    $0.append("fresh:\(action.payloadJSON)")
                }
                delivered.fulfill()
            }
            XCTAssertEqual(
                executor.tryEnqueue(
                    request: Self.request(payloadJSON: #"{"value":"fresh"}"#)
                ),
                .accepted
            )
            DispatchQueue.main.async { drained.fulfill() }
        }

        wait(for: [delivered, drained], timeout: 2)
        XCTAssertEqual(
            received.value,
            [#"fresh:{"value":"fresh"}"#]
        )
    }

    func testCloseIsIdempotentAndPurgesQueuedActions() {
        let executor = MacOSIncActionExecutor()
        let received = LockedValue<[NativeWorkbenchAction]>([])
        let results = LockedValue<[NativeIncActionEnqueueResult]>([])
        let drained = expectation(description: "main action queue drained")
        executor.setHandler { action in
            received.withValue { $0.append(action) }
        }

        DispatchQueue.main.async {
            results.withValue {
                $0.append(
                    executor.tryEnqueue(
                        request: Self.request(
                            payloadJSON: #"{"value":"before-close"}"#
                        )
                    )
                )
            }
            executor.close()
            executor.close()
            executor.setHandler { action in
                received.withValue { $0.append(action) }
            }
            results.withValue {
                $0.append(
                    executor.tryEnqueue(
                        request: Self.request(
                            payloadJSON: #"{"value":"after-close"}"#
                        )
                    )
                )
            }
            DispatchQueue.main.async { drained.fulfill() }
        }

        wait(for: [drained], timeout: 2)
        XCTAssertEqual(results.value, [.accepted, .closed])
        XCTAssertTrue(received.value.isEmpty)
    }

    func testProjectsEverySupportedKindAndExactPayload() {
        let executor = MacOSIncActionExecutor()
        let received = LockedValue<[NativeWorkbenchAction]>([])
        let delivered = expectation(description: "all action kinds delivered")
        delivered.expectedFulfillmentCount = 3
        let drained = expectation(description: "main action queue drained")
        let requests = [
            Self.request(
                sessionID: 11,
                sourceWindowID: 21,
                kind: "note-open",
                payloadJSON: #"{"target":{"type":"event","id":"abc"}}"#
            ),
            Self.request(
                sessionID: 12,
                sourceWindowID: 22,
                kind: "profile-open",
                payloadJSON: #"{"pubkey":"def"}"#
            ),
            Self.request(
                sessionID: 13,
                sourceWindowID: 23,
                kind: "compose-open",
                payloadJSON: #"{"intent":"reply","replyTo":{"id":"ghi"}}"#
            ),
        ]
        executor.setHandler { action in
            received.withValue { $0.append(action) }
            delivered.fulfill()
        }

        DispatchQueue.main.async {
            for request in requests {
                XCTAssertEqual(
                    executor.tryEnqueue(request: request),
                    .accepted
                )
            }
            DispatchQueue.main.async { drained.fulfill() }
        }

        wait(for: [delivered, drained], timeout: 2)
        XCTAssertEqual(
            received.value,
            [
                NativeWorkbenchAction(
                    manifestAuthor: Self.manifestAuthor,
                    dTag: "good-morning",
                    aggregateHash: Self.aggregateHash,
                    sessionID: 11,
                    sourceWindowID: 21,
                    kind: .noteOpen,
                    payloadJSON:
                        #"{"target":{"type":"event","id":"abc"}}"#
                ),
                NativeWorkbenchAction(
                    manifestAuthor: Self.manifestAuthor,
                    dTag: "good-morning",
                    aggregateHash: Self.aggregateHash,
                    sessionID: 12,
                    sourceWindowID: 22,
                    kind: .profileOpen,
                    payloadJSON: #"{"pubkey":"def"}"#
                ),
                NativeWorkbenchAction(
                    manifestAuthor: Self.manifestAuthor,
                    dTag: "good-morning",
                    aggregateHash: Self.aggregateHash,
                    sessionID: 13,
                    sourceWindowID: 23,
                    kind: .composeOpen,
                    payloadJSON:
                        #"{"intent":"reply","replyTo":{"id":"ghi"}}"#
                ),
            ]
        )
    }

    private static let manifestAuthor = String(repeating: "a", count: 64)
    private static let aggregateHash = String(repeating: "b", count: 64)

    private static func request(
        sessionID: UInt64 = 1,
        sourceWindowID: UInt64 = 10,
        kind: String = "profile-open",
        payloadJSON: String = #"{"pubkey":"abc"}"#
    ) -> NativeIncActionRequest {
        NativeIncActionRequest(
            manifestAuthor: manifestAuthor,
            dTag: "good-morning",
            aggregateHash: aggregateHash,
            sessionId: sessionID,
            sourceWindowId: sourceWindowID,
            kind: kind,
            payloadJson: payloadJSON
        )
    }

    private static func end(
        sessionID: UInt64,
        sourceWindowID: UInt64
    ) -> NativeIncActionEnd {
        NativeIncActionEnd(
            manifestAuthor: manifestAuthor,
            dTag: "good-morning",
            aggregateHash: aggregateHash,
            sessionId: sessionID,
            sourceWindowId: sourceWindowID,
            reason: "closed-stopped"
        )
    }
}

private final class LockedValue<Value>: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: Value

    init(_ value: Value) {
        storage = value
    }

    var value: Value {
        lock.withLock { storage }
    }

    func set(_ value: Value) {
        lock.withLock { storage = value }
    }

    func withValue<Result>(
        _ body: (inout Value) throws -> Result
    ) rethrows -> Result {
        try lock.withLock { try body(&storage) }
    }
}
