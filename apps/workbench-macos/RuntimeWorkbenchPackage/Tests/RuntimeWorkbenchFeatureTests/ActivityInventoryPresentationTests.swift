@testable import RuntimeWorkbenchFeature
import Testing

@Test func activityInventoryShowsOnlyTheGenuinelyScopedSessionCount() {
    let inventory = ActivityInventorySummary(
        activeSessions: 0,
        activeBindings: 7,
        activeResources: 8,
        pendingReceipts: 9
    )!
    let presentation = ActivityInventoryPresentation(inventory: inventory)

    #expect(presentation.openNow == 0)
    #expect(
        presentation.unavailableCountsMessage
            == "Other activity counts aren't available in this version."
    )
}

@Test func activityCopyNeverPromisesCompleteness() {
    let copy = [
        ActivityPlainPresentation.header,
        ActivityPlainPresentation.updateGap,
    ].joined(separator: " ")

    #expect(!copy.lowercased().contains("everything"))
    #expect(!copy.lowercased().contains("complete"))
}
