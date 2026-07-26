@testable import RuntimeWorkbenchFeature
import Testing

@Test func removalCopyNamesOnlyTheStateUninstallActuallyOwns() {
    let message = WorkbenchLibraryRemovalPresentation.message(
        for: "Good Morning"
    )

    #expect(message.contains("Good Morning will be removed from your library"))
    #expect(message.contains("permissions"))
    #expect(message.contains("saved napplet data"))
    #expect(message.contains("workspace placements"))
    #expect(message.contains("Activity history"))
    #expect(message.contains("receipts"))
    #expect(message.contains("workspace definitions"))
    #expect(message.contains("downloaded build files will remain"))
    #expect(!message.contains("everything"))
}
