@testable import RuntimeWorkbenchFeature
import NMPNativeRuntimeApple
import Testing

@Test func everyTypedReceiptOutcomeHasAnExplicitVerdict() {
    let cases: [(NativeRuntimeReceiptOutcome, String)] = [
        (.inProgress, "Delivery in progress"),
        (.delivered, "Delivered"),
        (.partialDelivery, "Partially delivered"),
        (.exhausted, "Delivery not confirmed"),
        (.ambiguous, "Delivery outcome unknown"),
        (.refused, "Delivery refused"),
        (.failed, "Delivery failed"),
        (.cancelled, "Delivery cancelled"),
        (.conflict, "Delivery conflict"),
        (.unavailable, "Delivery status unavailable"),
    ]
    for (outcome, expectedTitle) in cases {
        let presentation = ReceiptStatusPresentation(
            receiptID: "receipt-42",
            outcome: outcome,
            observationLifecycle: .observing,
            outcomeDetail: nil,
            latestStateJSON: nil
        )
        #expect(presentation.title == expectedTitle)
    }
}

@Test func lifecycleDoesNotChangeTheDurableVerdict() {
    for lifecycle: NativeRuntimeReceiptObservationLifecycle in [
        .observing,
        .notFound,
        .closed,
    ] {
        let presentation = ReceiptStatusPresentation(
            receiptID: "receipt-42",
            outcome: .delivered,
            observationLifecycle: lifecycle,
            outcomeDetail: nil,
            latestStateJSON: nil
        )
        #expect(presentation.title == "Delivered")
    }
}

@Test func receiptEvidencePreservesTypedDetailAndOpaqueNMPState() {
    let state = #"{"schema":"nostr.write.receipt/1","state":"partial_delivery","relays":{"wss://one.example":{"state":"rejected","reason":"policy"}}}"#
    let presentation = ReceiptStatusPresentation(
        receiptID: "receipt-42",
        outcome: .partialDelivery,
        observationLifecycle: .closed,
        outcomeDetail: "one relay rejected the write",
        latestStateJSON: state
    )
    let fields = presentation.evidenceFields

    #expect(fields.map(\.label) == [
        "Receipt id",
        "Durable outcome",
        "Observation lifecycle",
        "Outcome detail",
        "Latest NMP state",
    ])
    #expect(fields.map(\.value) == [
        "receipt-42",
        "partial delivery",
        "closed",
        "one relay rejected the write",
        state,
    ])
}
