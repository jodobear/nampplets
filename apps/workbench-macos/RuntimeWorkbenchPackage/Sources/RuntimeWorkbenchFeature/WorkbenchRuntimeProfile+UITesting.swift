import Foundation

extension WorkbenchRuntimeProfile {
    /// Opens a fresh, transient profile for one named UI-test scenario.
    ///
    /// The root remains inside the app's sandboxed temporary directory, but
    /// `WorkbenchUITestStorage` scopes it to this run so persisted grants and
    /// accounts cannot leak between runs and concurrent runs on one machine
    /// cannot clear each other's profile.
    public static func openForUITesting(
        scenario: String
    ) throws -> WorkbenchRuntimeProfile {
        let profile = try open(
            storageRoot: WorkbenchUITestStorage.prepareStorageRoot(
                scenario: scenario
            ),
            accountPersistence: .transient
        )
        if scenario == "full-window-layout-transition" {
            let registration = profile.native.registerLocalAccount(
                secretKey: String(repeating: "0", count: 63) + "1"
            )
            guard
                registration.accepted,
                let handle = registration.handle,
                profile.native.activateLocalAccount(handle: handle).accepted
            else {
                profile.close()
                throw CocoaError(.userCancelled)
            }
        }
        return profile
    }
}
