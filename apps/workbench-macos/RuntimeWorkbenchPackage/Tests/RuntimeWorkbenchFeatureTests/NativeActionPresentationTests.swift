import NMPNativeRuntimeApple
import Testing
@testable import RuntimeWorkbenchFeature

struct NativeActionPresentationTests {
    private let author = String(repeating: "a", count: 64)
    private let aggregate = String(repeating: "b", count: 64)

    @Test
    func notePresentationDoesNotInterpretPayload() {
        let payload = #"{"target":{"id":"not-validated-in-swift"}}"#
        let action = NativeWorkbenchAction(
            manifestAuthor: author,
            dTag: "good-morning",
            aggregateHash: aggregate,
            sessionID: 4,
            sourceWindowID: 8,
            kind: .noteOpen,
            payloadJSON: payload
        )

        let notice = NativeActionNotice.presentation(action)

        #expect(notice.title == "Open a post")
        #expect(notice.summary == "This napplet wants to show you a post.")
        #expect(notice.evidence.contains {
            $0.label == "Payload JSON" && $0.value == payload
        })
        #expect(notice.evidence.contains {
            $0.label == "Action kind" && $0.value == "note-open"
        })
        #expect(notice.evidence.contains {
            $0.label == "Manifest author" && $0.value == author
        })
        #expect(notice.evidence.contains {
            $0.label == "dTag" && $0.value == action.dTag
        })
        #expect(notice.evidence.contains {
            $0.label == "Aggregate hash" && $0.value == aggregate
        })
        #expect(notice.evidence.contains {
            $0.label == "Source session" && $0.value == "4"
        })
        #expect(notice.evidence.contains {
            $0.label == "Source window" && $0.value == "8"
        })
    }

    @Test
    func malformedAndNovelPayloadsCannotChangeThePlainVerdict() {
        let malformed = NativeWorkbenchAction(
            manifestAuthor: author,
            dTag: "good-morning",
            aggregateHash: aggregate,
            sessionID: 4,
            sourceWindowID: 8,
            kind: .profileOpen,
            payloadJSON: #"{"pubkey":"not-a-pubkey"}"#
        )
        let novel = NativeWorkbenchAction(
            manifestAuthor: author,
            dTag: "good-morning",
            aggregateHash: aggregate,
            sessionID: 4,
            sourceWindowID: 8,
            kind: .profileOpen,
            payloadJSON: #"["novel",{"shape":true}]"#
        )

        let first = NativeActionNotice.presentation(malformed)
        let second = NativeActionNotice.presentation(novel)
        #expect(first.title == second.title)
        #expect(first.summary == second.summary)
        #expect(first.evidence.last?.value == malformed.payloadJSON)
        #expect(second.evidence.last?.value == novel.payloadJSON)
    }

    @Test
    func composePresentationDoesNotInferReplySemantics() {
        let action = NativeWorkbenchAction(
            manifestAuthor: author,
            dTag: "good-morning",
            aggregateHash: aggregate,
            sessionID: 4,
            sourceWindowID: 8,
            kind: .composeOpen,
            payloadJSON: #"{"replyTo":{"id":"opaque"}}"#
        )

        let notice = NativeActionNotice.presentation(action)
        #expect(notice.title == "Write something")
        #expect(notice.summary.contains("write a post"))
        #expect(!notice.summary.contains("reply"))
        #expect(notice.evidence.last?.value == action.payloadJSON)
    }
}
