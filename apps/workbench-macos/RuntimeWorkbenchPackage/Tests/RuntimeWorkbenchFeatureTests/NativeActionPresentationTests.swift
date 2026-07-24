import NMPNativeRuntimeApple
import Testing
@testable import RuntimeWorkbenchFeature

struct NativeActionPresentationTests {
    private let author = String(repeating: "a", count: 64)
    private let aggregate = String(repeating: "b", count: 64)

    @Test
    func notePayloadProjectsOnlyTheBoundedEventTarget() {
        let eventID = String(repeating: "c", count: 64)
        let action = NativeWorkbenchAction(
            manifestAuthor: author,
            dTag: "good-morning",
            aggregateHash: aggregate,
            sessionID: 4,
            sourceWindowID: 8,
            kind: .noteOpen,
            payloadJSON: "{\"target\":{\"type\":\"event\",\"id\":\"\(eventID)\",\"kind\":1,\"pubkey\":\"\(author)\"},\"extra\":{\"secret\":\"ignored\"}}"
        )

        let notice = NativeActionNotice.decode(action)

        #expect(notice?.title == "Note requested")
        #expect(notice?.target.contains("kind 1") == true)
        #expect(notice?.detail.contains(author) == true)
    }

    @Test
    func malformedProfilePayloadFailsClosed() {
        let action = NativeWorkbenchAction(
            manifestAuthor: author,
            dTag: "good-morning",
            aggregateHash: aggregate,
            sessionID: 4,
            sourceWindowID: 8,
            kind: .profileOpen,
            payloadJSON: #"{"pubkey":"not-a-pubkey"}"#
        )

        #expect(NativeActionNotice.decode(action) == nil)
    }

    @Test
    func composePayloadIsDisplayedWithoutCreatingAComposer() {
        let eventID = String(repeating: "d", count: 64)
        let action = NativeWorkbenchAction(
            manifestAuthor: author,
            dTag: "good-morning",
            aggregateHash: aggregate,
            sessionID: 4,
            sourceWindowID: 8,
            kind: .composeOpen,
            payloadJSON: "{\"intent\":\"reply\",\"replyTo\":{\"id\":\"\(eventID)\"}}"
        )

        let notice = NativeActionNotice.decode(action)

        #expect(notice?.title == "Compose requested")
        #expect(notice?.detail == "The Workbench does not provide a composer.")
        #expect(notice?.target.contains(eventID) == true)
    }
}
