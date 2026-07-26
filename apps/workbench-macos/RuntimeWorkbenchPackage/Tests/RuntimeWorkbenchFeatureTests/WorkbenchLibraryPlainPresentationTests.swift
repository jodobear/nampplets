@testable import RuntimeWorkbenchFeature
import Testing

@Test func libraryPlainStatusNeverUsesRuntimeDiagnostics() {
    let messages = [
        WorkbenchLibraryPlainPresentation.notReadyMessage,
        WorkbenchLibraryPlainPresentation.unavailableMessage,
        WorkbenchLibraryPlainPresentation.refusalMessage,
    ]
    let forbidden = [
        "sealed exact-build bytes",
        "exact aggregate",
        "projection",
        "revision",
    ]

    for message in messages {
        for term in forbidden {
            #expect(!message.lowercased().contains(term.lowercased()))
        }
    }
}
