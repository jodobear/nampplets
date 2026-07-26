@testable import RuntimeWorkbenchFeature
import Testing

@Test func permissionSubtitleMakesNoFirstRunOrOpeningClaim() {
    let copy = [
        PermissionPlainCopy.subtitle(
            hasCapabilities: true,
            isManagedReviewBlocked: false
        ),
        PermissionPlainCopy.subtitle(
            hasCapabilities: true,
            isManagedReviewBlocked: true
        ),
        PermissionPlainCopy.subtitle(
            hasCapabilities: false,
            isManagedReviewBlocked: false
        ),
    ]

    #expect(
        copy == [
            "Review what this napplet can do and any choices available here.",
            "Review what this napplet can do and any choices available here.",
            "There are no access choices in this review.",
        ]
    )
    for sentence in copy {
        #expect(!sentence.localizedCaseInsensitiveContains("first"))
        #expect(!sentence.localizedCaseInsensitiveContains("opening"))
    }
}
