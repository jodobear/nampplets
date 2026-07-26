@testable import RuntimeWorkbenchFeature
import Testing

@Test func nonKindOneConsentKeepsTheEntireExactDraftOnPath() {
    let draft = #"{"kind":0,"content":"{\"name\":\"Alice\"}","tags":[["alt","profile metadata"]],"created_at":42}"#
    let presentation = PendingWriteConsentPresentation(exactDraft: draft)

    #expect(presentation.exactDraft == draft)
    #expect(presentation.exactDraft.contains(#""kind":0"#))
    #expect(presentation.exactDraft.contains(#""tags""#))
}

@Test func taggedKindOneConsentIsNeverReducedToContent() {
    let draft = #"{"kind":1,"content":"Looks harmless","tags":[["e","event-to-reply-to"],["p","person-being-addressed"]]}"#
    let presentation = PendingWriteConsentPresentation(exactDraft: draft)

    #expect(presentation.exactDraft == draft)
    #expect(presentation.exactDraft != "Looks harmless")
    #expect(presentation.exactDraft.contains("event-to-reply-to"))
    #expect(presentation.exactDraft.contains("person-being-addressed"))
}
