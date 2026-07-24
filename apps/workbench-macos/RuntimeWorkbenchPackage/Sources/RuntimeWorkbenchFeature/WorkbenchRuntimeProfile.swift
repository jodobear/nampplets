import Foundation
import NMPNativeRuntimeApple

/// Application-owned wrapper for one local Workbench trust profile.
///
/// Every window and workspace slot borrows this same native profile. The app,
/// rather than any napplet view, owns final shutdown.
public final class WorkbenchRuntimeProfile: @unchecked Sendable {
    let native: NativeRuntimeProfile

    public static func openDefault() throws -> WorkbenchRuntimeProfile {
        let base = try FileManager.default.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        )
        let storageRoot = base
            .appendingPathComponent(
                "io.f7z.nmp.native-runtime.workbench",
                isDirectory: true
            )
            .appendingPathComponent("runtime", isDirectory: true)
        return try open(
            storageRoot: storageRoot,
            accountPersistence: keychainPersistence(
                storageRoot: storageRoot
            )
        )
    }

    /// Opens a fresh, transient profile for one named UI-test scenario.
    ///
    /// The root remains inside the app's sandboxed temporary directory. A
    /// finite scenario name selects one reusable directory, which is cleared
    /// before launch so persisted grants and accounts cannot leak between
    /// developer runs or CI machines.
    public static func openForUITesting(
        scenario: String
    ) throws -> WorkbenchRuntimeProfile {
        guard
            !scenario.isEmpty,
            scenario.utf8.count <= 64,
            scenario.unicodeScalars.allSatisfy({
                (CharacterSet.lowercaseLetters.contains($0)
                    || CharacterSet.decimalDigits.contains($0)
                    || $0 == "-")
                    && $0.isASCII
            })
        else {
            throw CocoaError(.fileReadInvalidFileName)
        }
        let storageRoot = FileManager.default.temporaryDirectory
            .appendingPathComponent(
                "io.f7z.nmp.native-runtime.workbench-ui-tests",
                isDirectory: true
            )
            .appendingPathComponent(scenario, isDirectory: true)
        if FileManager.default.fileExists(atPath: storageRoot.path) {
            try FileManager.default.removeItem(at: storageRoot)
        }
        return try open(
            storageRoot: storageRoot,
            accountPersistence: .transient
        )
    }

    static func open(
        storageRoot: URL,
        accountPersistence: NativeRuntimeAccountPersistence = .transient
    ) throws -> WorkbenchRuntimeProfile {
        let native = try NativeRuntimeProfile.open(
            configuration: NativeRuntimeProfileConfiguration(
                storageRoot: storageRoot,
                accountPersistence: accountPersistence
            )
        )
        return WorkbenchRuntimeProfile(native: native)
    }

    static func keychainPersistence(
        storageRoot: URL
    ) -> NativeRuntimeAccountPersistence {
        .keychain(namespace: storageRoot.standardizedFileURL.path)
    }

    init(native: NativeRuntimeProfile) {
        self.native = native
    }

    public func close() {
        native.close()
    }

    deinit {
        close()
    }
}
