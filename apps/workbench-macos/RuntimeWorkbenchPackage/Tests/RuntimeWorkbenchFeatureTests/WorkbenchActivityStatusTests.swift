import Testing
@testable import RuntimeWorkbenchFeature

@Test func activityFailuresKeepRawDetailsInEvidenceOnly() {
    let raw = "runtime exact-build principal refused projection"
    for status: WorkbenchActivityStatus in [
        .refused(detail: raw),
        .failed(detail: raw),
    ] {
        let presentation = WorkbenchActivityBarPresentation(
            status: status,
            layoutPersistenceError: nil,
            capacityWarning: nil
        )

        #expect(!status.message.contains(raw))
        #expect(presentation.evidenceFields.map(\.value) == [raw])
    }
}

@Test func layoutFailuresHaveGenericCopyAndExactEvidence() {
    let persistenceRaw = "sqlite write failed at workspace_revision=41"
    let persistence = WorkbenchActivityBarPresentation(
        status: .ready,
        layoutPersistenceError: persistenceRaw,
        capacityWarning: nil
    )
    #expect(
        persistence.layoutMessages == ["Layout changes couldn't be saved."]
    )
    #expect(persistence.evidenceFields.map(\.value) == [persistenceRaw])

    let capacityRaw = "layout capacity 32 refused window 33"
    let capacity = WorkbenchActivityBarPresentation(
        status: .ready,
        layoutPersistenceError: nil,
        capacityWarning: capacityRaw
    )
    #expect(
        capacity.layoutMessages
            == ["Some saved windows couldn't be restored."]
    )
    #expect(capacity.evidenceFields.map(\.value) == [capacityRaw])

    let simultaneous = WorkbenchActivityBarPresentation(
        status: .ready,
        layoutPersistenceError: persistenceRaw,
        capacityWarning: capacityRaw
    )
    #expect(simultaneous.layoutMessages.count == 2)
    #expect(
        simultaneous.evidenceFields.map(\.value)
            == [persistenceRaw, capacityRaw]
    )
    #expect(
        simultaneous.policyMessage
            == "Napplet access follows your choices and managed settings."
    )
}

@Test func normalActivityMessagesRemainSpecificAndNonTechnical() {
    #expect(WorkbenchActivityStatus.preparing.message == "Getting things ready")
    #expect(
        WorkbenchActivityStatus.restoring(count: 2).message
            == "Reopening 2 saved napplets"
    )
    #expect(
        WorkbenchActivityStatus.running(title: "Notes").message
            == "Notes is running"
    )
    #expect(
        WorkbenchActivityStatus.crashed(title: "Notes").message
            == "Notes stopped unexpectedly"
    )
}
