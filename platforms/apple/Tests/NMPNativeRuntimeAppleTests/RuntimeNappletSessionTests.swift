import Foundation
import NMPNativeRuntime
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

        let profile = try NativeRuntimeProfile.open(
            configuration: NativeRuntimeProfileConfiguration(storageRoot: root)
        )
        defer { profile.close() }
        let artifact = try profile.openSignedNamed(
            title: "Good Morning Protocol",
            eventJSON: event,
            author: author,
            dTag: "good-morning",
            blobsBySHA256: [indexDigest: index],
            grantDomains: ["storage"]
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

    func testStoppingOneSessionDoesNotCloseSharedProfileOrSibling() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(
                "runtime-apple-shared-profile-\(UUID().uuidString)",
                isDirectory: true
            )
        defer { try? FileManager.default.removeItem(at: root) }
        let fixture = repositoryRoot().appendingPathComponent(
            "conformance/napplet-corpus/published/good-morning",
            isDirectory: true
        )
        let event = try Data(
            contentsOf: fixture.appendingPathComponent("event.json")
        )
        let index = try Data(
            contentsOf: fixture.appendingPathComponent("index.html")
        )
        let profile = try NativeRuntimeProfile.open(
            configuration: NativeRuntimeProfileConfiguration(storageRoot: root)
        )
        defer { profile.close() }

        let first = try profile.openSignedNamed(
            title: "Good Morning One",
            eventJSON: event,
            author: author,
            dTag: "good-morning",
            blobsBySHA256: [indexDigest: index],
            grantDomains: ["storage"]
        )
        let second = try profile.openSignedNamed(
            title: "Good Morning Two",
            eventJSON: event,
            author: author,
            dTag: "good-morning",
            blobsBySHA256: [indexDigest: index],
            grantDomains: ["storage"]
        )
        let firstRuntime = try XCTUnwrap(first.runtimeSession)
        let secondRuntime = try XCTUnwrap(second.runtimeSession)
        XCTAssertNotEqual(firstRuntime.sessionID, secondRuntime.sessionID)

        firstRuntime.stop()
        XCTAssertFalse(profile.snapshotForTesting.closed)
        XCTAssertEqual(
            profile.snapshotForTesting.sessions.first(where: {
                $0.id == secondRuntime.sessionID
            })?.state,
            "running"
        )

        let received = expectation(
            description: "Sibling still receives provider responses"
        )
        secondRuntime.setResponseSink { bytes in
            guard let envelope = try? JSONSerialization.jsonObject(with: bytes)
                    as? [String: Any],
                  envelope["type"] as? String == "shell.init"
            else {
                return
            }
            received.fulfill()
        }
        secondRuntime.mappedEnvelope(Data(#"{"type":"shell.ready"}"#.utf8))
        wait(for: [received], timeout: 2)
        secondRuntime.stop()
    }

    func testClosingProfileInvalidatesEveryBorrowedSession() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(
                "runtime-apple-profile-close-\(UUID().uuidString)",
                isDirectory: true
            )
        defer { try? FileManager.default.removeItem(at: root) }
        let fixture = repositoryRoot().appendingPathComponent(
            "conformance/napplet-corpus/published/good-morning",
            isDirectory: true
        )
        let event = try Data(
            contentsOf: fixture.appendingPathComponent("event.json")
        )
        let index = try Data(
            contentsOf: fixture.appendingPathComponent("index.html")
        )
        let profile = try NativeRuntimeProfile.open(
            configuration: NativeRuntimeProfileConfiguration(storageRoot: root)
        )
        let artifact = try profile.openSignedNamed(
            title: "Good Morning Close",
            eventJSON: event,
            author: author,
            dTag: "good-morning",
            blobsBySHA256: [indexDigest: index],
            grantDomains: ["storage"]
        )
        let runtime = try XCTUnwrap(artifact.runtimeSession)
        let unexpectedResponse = expectation(
            description: "Closed sessions cannot receive responses"
        )
        unexpectedResponse.isInverted = true
        runtime.setResponseSink { _ in unexpectedResponse.fulfill() }

        profile.close()

        XCTAssertTrue(profile.snapshotForTesting.closed)
        XCTAssertNil(
            try runtime.readSealed(logicalPath: "/index.html")
        )
        runtime.mappedEnvelope(Data(#"{"type":"shell.ready"}"#.utf8))
        wait(for: [unexpectedResponse], timeout: 0.1)
    }

    func testProfileRegistersNativeThemeAndConfigCapabilities() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(
                "runtime-apple-native-providers-\(UUID().uuidString)",
                isDirectory: true
            )
        defer { try? FileManager.default.removeItem(at: root) }
        let fixture = repositoryRoot().appendingPathComponent(
            "conformance/napplet-corpus/published/good-morning",
            isDirectory: true
        )
        let event = try Data(contentsOf: fixture.appendingPathComponent("event.json"))
        let index = try Data(contentsOf: fixture.appendingPathComponent("index.html"))
        let profile = try NativeRuntimeProfile.open(
            configuration: NativeRuntimeProfileConfiguration(storageRoot: root)
        )
        defer { profile.close() }
        let artifact = try profile.openSignedNamed(
            title: "Good Morning Native Providers",
            eventJSON: event,
            author: author,
            dTag: "good-morning",
            blobsBySHA256: [indexDigest: index],
            grantDomains: ["theme", "config"]
        )
        XCTAssertTrue(artifact.negotiatedDomains.contains("theme"))
        XCTAssertTrue(artifact.negotiatedDomains.contains("config"))
        let runtime = try XCTUnwrap(artifact.runtimeSession)
        defer { runtime.stop() }

        let themeReceived = expectation(description: "native theme response")
        let configReceived = expectation(description: "persisted config defaults")
        runtime.setResponseSink { bytes in
            guard let envelope = try? JSONSerialization.jsonObject(with: bytes)
                    as? [String: Any],
                  let type = envelope["type"] as? String
            else {
                return
            }
            if type == "theme.get.result",
               let theme = envelope["theme"] as? [String: Any],
               let colors = theme["colors"] as? [String: String],
               colors["background"]?.hasPrefix("#") == true,
               colors["text"]?.hasPrefix("#") == true,
               colors["primary"]?.hasPrefix("#") == true
            {
                themeReceived.fulfill()
            }
            if type == "config.values",
               let values = envelope["values"] as? [String: Any],
               values["enabled"] as? Bool == true
            {
                configReceived.fulfill()
            }
        }
        runtime.mappedEnvelope(Data(#"{"type":"shell.ready"}"#.utf8))
        runtime.mappedEnvelope(Data(#"{"type":"theme.get","id":"theme-1"}"#.utf8))
        runtime.mappedEnvelope(
            Data(
                #"{"type":"config.registerSchema","id":"schema-1","schema":{"$version":1,"type":"object","properties":{"enabled":{"type":"boolean","default":true}},"additionalProperties":false},"version":1}"#.utf8
            )
        )
        runtime.mappedEnvelope(Data(#"{"type":"config.get","id":"config-1"}"#.utf8))
        wait(for: [themeReceived, configReceived], timeout: 2)
    }

    func testNativeSettingsDocumentFailsClosedForInvalidOrOversizedJSON() {
        let request = NativeSettingsRequest(
            manifestAuthor: String(repeating: "a", count: 64),
            dTag: "settings",
            aggregateHash: String(repeating: "b", count: 64),
            sessionId: 7,
            section: nil,
            schemaJson: #"{"type":"object","properties":{}}"#,
            valuesJson: "{}"
        )
        XCTAssertNotNil(NativeSettingsDocument.decode(request))
        var invalid = request
        invalid.schemaJson = "[]"
        XCTAssertNil(NativeSettingsDocument.decode(invalid))
        invalid = request
        invalid.valuesJson = String(repeating: "x", count: 192 * 1_024 + 1)
        XCTAssertNil(NativeSettingsDocument.decode(invalid))
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
