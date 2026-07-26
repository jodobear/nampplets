@testable import RuntimeWorkbenchFeature
import Testing

@Test func titlelessManifestUsesGenericTitleInsteadOfAProtocolIdentifier() {
    #expect(
        CatalogTitlePresentation.displayTitle(nil) == "Untitled napplet"
    )
    #expect(
        CatalogTitlePresentation.displayTitle("") == "Untitled napplet"
    )
    #expect(
        CatalogTitlePresentation.displayTitle("   \n") == "Untitled napplet"
    )
    #expect(
        CatalogTitlePresentation.displayTitle("Good Morning") == "Good Morning"
    )
    #expect(
        CatalogTitlePresentation.displayTitle("  Good Morning ")
            == "  Good Morning "
    )
    #expect(
        CatalogTitlePresentation.displayTitle(nil) != "good-morning-d-tag"
    )
}
