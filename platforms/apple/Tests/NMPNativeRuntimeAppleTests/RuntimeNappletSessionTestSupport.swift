import Foundation
import NMPNativeRuntime
import XCTest
import WebKit
@testable import NMPNativeRuntimeApple

// MARK: - Shared fixture coordinates and bounded test recorders

class RuntimeNappletSessionTestCase: XCTestCase {
    let author =
        "266815e0c9210dfa324c6cba3573b14bee49da4209a9456f9484e5106cd408a5"
    let indexDigest =
        "ffd35eea5c84d03cdda74c23e1bbb2c40500f503833503aa688036faa52f3808"
    /// Every domain the published fixture's own `napplet-requires` meta
    /// declares. All of them are required -- no runtime code softens any of
    /// them to optional on the strength of this build's identity -- so a test
    /// that launches this fixture has to grant the whole set.
    ///
    /// `link` and `resource` have no registered provider, so `RuntimeApp`
    /// drops them with a `required-domain-unavailable` activity rather than
    /// blocking. `theme` does have one, so leaving it ungranted genuinely
    /// refuses the launch.
    let requiredGoodMorningDomains = [
        "identity", "inc", "link", "outbox", "resource", "theme",
    ]

    func repositoryRoot() -> URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
    }

    func librarySnapshot(
        _ projection: NativeRuntimeLibraryProjection
    ) throws -> NativeRuntimeLibrarySnapshot {
        guard case let .snapshot(snapshot) = projection else {
            XCTFail("Expected a complete installed-library snapshot")
            throw RuntimeNappletSessionTestError.expectedLibrarySnapshot
        }
        return snapshot
    }
}

private enum RuntimeNappletSessionTestError: Error {
    case expectedLibrarySnapshot
}

final class LockedPendingWriteUpdates: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: [NativeRuntimePendingWriteUpdate] = []

    var values: [NativeRuntimePendingWriteUpdate] {
        lock.lock()
        defer { lock.unlock() }
        return storage
    }

    func append(_ update: NativeRuntimePendingWriteUpdate) {
        lock.lock()
        storage.append(update)
        lock.unlock()
    }
}

final class LockedReceiptUpdates: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: [NativeRuntimeReceiptUpdate] = []

    var values: [NativeRuntimeReceiptUpdate] {
        lock.lock()
        defer { lock.unlock() }
        return storage
    }

    func append(_ update: NativeRuntimeReceiptUpdate) {
        lock.lock()
        storage.append(update)
        lock.unlock()
    }
}
