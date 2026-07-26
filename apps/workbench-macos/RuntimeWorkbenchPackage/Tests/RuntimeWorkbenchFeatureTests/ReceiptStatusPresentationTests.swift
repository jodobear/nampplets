@testable import RuntimeWorkbenchFeature
import Testing

@Test func receiptStatusMakesNoPublicationOrLifecycleClaim() {
    for lifecycle in [
        "observing",
        "notfound",
        "closed",
        "",
        "future-state",
    ] {
        let presentation = ReceiptStatusPresentation(
            receiptID: "receipt-42",
            status: .delivered,
            delivery: lifecycle,
            latestStateJSON: nil
        )

        #expect(presentation.title == "Delivery details available")
        for forbidden in ["update", "posted", "pending", "stopped"] {
            #expect(!presentation.title.lowercased().contains(forbidden))
        }
    }
}

@Test func receiptEvidencePreservesOpaqueNMPStateWithoutInterpretingIt() {
    let state = #"{"schema":"nostr.write.receipt/1","state":"partial_delivery","relays":{"wss://one.example":{"state":"rejected","reason":"policy"}}}"#
    let presentation = ReceiptStatusPresentation(
        receiptID: "receipt-42",
        status: .pending,
        delivery: "closed",
        latestStateJSON: state
    )
    let fields = presentation.evidenceFields

    #expect(fields.map(\.label) == [
        "Receipt id",
        "Runtime status",
        "Observation lifecycle",
        "Latest NMP state",
    ])
    #expect(fields.map(\.value) == ["receipt-42", "pending", "closed", state])
}
