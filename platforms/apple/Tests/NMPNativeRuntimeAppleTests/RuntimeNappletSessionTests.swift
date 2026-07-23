import Foundation
import XCTest
@testable import NMPNativeRuntimeApple

final class RuntimeNappletSessionTests: XCTestCase {
    private let author =
        "266815e0c9210dfa324c6cba3573b14bee49da4209a9456f9484e5106cd408a5"
    private let indexDigest =
        "ffd35eea5c84d03cdda74c23e1bbb2c40500f503833503aa688036faa52f3808"

    func testSignedNamedArtifactNegotiatesAndRespondsThroughRust() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(
                "runtime-apple-test-\(UUID().uuidString)",
                isDirectory: true
            )
        defer { try? FileManager.default.removeItem(at: root) }
        let repository = repositoryRoot()
        let fixture = repository.appendingPathComponent(
            "conformance/napplet-corpus/published/good-morning",
            isDirectory: true
        )
        let event = try Data(
            contentsOf: fixture.appendingPathComponent("event.json")
        )
        let index = try Data(
            contentsOf: fixture.appendingPathComponent("index.html")
        )

        let artifact = try NappletArtifact.openSignedNamed(
            title: "Good Morning Protocol",
            eventJSON: event,
            author: author,
            dTag: "good-morning",
            blobsBySHA256: [indexDigest: index],
            grantDomains: ["storage"],
            storageRoot: root
        )
        let runtime = try XCTUnwrap(artifact.runtimeSession)
        defer { runtime.stop() }

        XCTAssertEqual(artifact.negotiatedDomains, ["shell", "storage"])
        let sealed = try XCTUnwrap(
            try artifact.reader.readSealed(logicalPath: "/index.html")
        )
        XCTAssertEqual(sealed.sha256, indexDigest)
        XCTAssertEqual(sealed.bytes, index)

        let received = expectation(description: "Rust emits the pinned shell.init")
        let response = LockedData()
        runtime.setResponseSink { bytes in
            response.set(bytes)
            received.fulfill()
        }
        runtime.mappedEnvelope(Data(#"{"type":"shell.ready"}"#.utf8))

        wait(for: [received], timeout: 2)
        let bytes = try XCTUnwrap(response.value)
        let envelope = try XCTUnwrap(
            JSONSerialization.jsonObject(with: bytes) as? [String: Any]
        )
        XCTAssertEqual(envelope["type"] as? String, "shell.init")
        let capabilities = try XCTUnwrap(
            envelope["capabilities"] as? [String: Any]
        )
        XCTAssertEqual(
            capabilities["domains"] as? [String],
            ["shell", "storage"]
        )
    }

    private func repositoryRoot() -> URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
    }
}

private final class LockedData: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: Data?

    var value: Data? {
        lock.lock()
        defer { lock.unlock() }
        return storage
    }

    func set(_ value: Data) {
        lock.lock()
        storage = value
        lock.unlock()
    }
}
