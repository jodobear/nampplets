@testable import RuntimeWorkbenchFeature
import Testing

@MainActor
@Test func repeatedEvidenceLabelsKeepOccurrenceStableRenderIdentity() {
    let grid = NappletFieldGrid(fields: [
        NappletField("Relay", "wss://one.example"),
        NappletField("Relay", "wss://two.example"),
    ])

    #expect(grid.occurrences.map(\.id) == [0, 1])
    #expect(grid.occurrences.map(\.field.label) == ["Relay", "Relay"])
    #expect(
        grid.occurrences.map(\.field.value)
            == ["wss://one.example", "wss://two.example"]
    )
}
