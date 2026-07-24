import SwiftUI
import Testing

@testable import RuntimeWorkbenchFeature

@MainActor
@Test
func settingsSnapshotReportsOpenProfileWithoutPathsOrSecrets() {
    let snapshot = WorkbenchSettingsSnapshot(profileAvailable: true)

    #expect(snapshot?.profileStatus == .open)
    #expect(snapshot?.profileStatus.detail.contains("/") == false)
}

@MainActor
@Test
func unavailableSettingsSnapshotRequiresBoundedDisplaySafeEvidence() {
    #expect(
        WorkbenchSettingsSnapshot(
            profileAvailable: false,
            unavailableReason: "Profile bootstrap was refused."
        )?.profileStatus
            == .unavailable(reason: "Profile bootstrap was refused.")
    )
    #expect(
        WorkbenchSettingsSnapshot(
            profileAvailable: false,
            unavailableReason: " "
        ) == nil
    )
    #expect(
        WorkbenchSettingsSnapshot(
            profileAvailable: false,
            unavailableReason: String(
                repeating: "x",
                count: WorkbenchSettingsSnapshot.maximumReasonUTF8Bytes + 1
            )
        ) == nil
    )
}

@MainActor
@Test
func settingsSheetBuildsWithNativeDestinationsOnly() {
    let snapshot = WorkbenchSettingsSnapshot(profileAvailable: true)!
    _ = WorkbenchSettingsSheet(snapshot: snapshot) { destination in
        switch destination {
        case .account, .installedLibrary, .activity:
            break
        }
    }
}

@MainActor
@Test
func settingsDestinationWaitsForDismissalAndIsConsumedExactlyOnce() {
    var route = WorkbenchSettingsRouteState()
    route.schedule(.installedLibrary)

    #expect(
        route.consumeAfterDismiss(settingsIsPresented: true) == nil
    )
    #expect(route.pendingDestination == .installedLibrary)
    #expect(
        route.consumeAfterDismiss(settingsIsPresented: false)
            == .installedLibrary
    )
    #expect(route.pendingDestination == nil)
    #expect(
        route.consumeAfterDismiss(settingsIsPresented: false) == nil
    )
}

@MainActor
@Test
func settingsRouteIsBoundedToOnePendingDestination() {
    var route = WorkbenchSettingsRouteState()
    route.schedule(.account)
    route.schedule(.activity)

    #expect(route.pendingDestination == .activity)
    #expect(
        route.consumeAfterDismiss(settingsIsPresented: false) == .activity
    )
}

@MainActor
@Test
func settingsDestinationsHaveStableAccessibilityIdentifiers() {
    #expect(
        Set(
            [
                WorkbenchSettingsDestination.account,
                .installedLibrary,
                .activity,
            ].map(\.accessibilityIdentifier)
        ) == [
            "settings-account",
            "settings-installed-library",
            "settings-activity",
        ]
    )
}
