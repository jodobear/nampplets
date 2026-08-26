import Foundation
import NMPNativeRuntime
import XCTest
@testable import NMPNativeRuntimeApple

final class NativeRuntimeProfileConfigurationTests: XCTestCase {
    func testCatalogDeadlineDefaultAndOverrideReachRuntimeOpen() throws {
        let defaultConfiguration = NativeRuntimeProfileConfiguration(
            storageRoot: FileManager.default.temporaryDirectory
        )
        XCTAssertEqual(
            defaultConfiguration.catalogOperationDeadlineMillis,
            15_000
        )

        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(
                "runtime-apple-catalog-deadline-\(UUID().uuidString)",
                isDirectory: true
            )
        defer { try? FileManager.default.removeItem(at: root) }

        XCTAssertThrowsError(
            try NativeRuntimeProfile.open(
                configuration: NativeRuntimeProfileConfiguration(
                    storageRoot: root,
                    catalogOperationDeadlineMillis: 600_001
                )
            )
        ) { error in
            guard let runtimeError = error as? RuntimeOpenError,
                  case let .InvalidConfig(detail) = runtimeError
            else {
                return XCTFail("expected Rust invalid-config refusal, got \(error)")
            }
            XCTAssertTrue(detail.contains("catalog_operation_deadline_millis"))
        }
    }
}
