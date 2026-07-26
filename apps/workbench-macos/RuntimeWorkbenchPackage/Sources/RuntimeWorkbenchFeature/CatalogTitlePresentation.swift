import Foundation

enum CatalogTitlePresentation {
    /// A protocol dTag is evidence, never a human-facing title.
    static func displayTitle(_ verifiedTitle: String?) -> String {
        guard
            let verifiedTitle,
            !verifiedTitle.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else {
            return "Untitled napplet"
        }
        return verifiedTitle
    }
}
