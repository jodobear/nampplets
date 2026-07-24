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
