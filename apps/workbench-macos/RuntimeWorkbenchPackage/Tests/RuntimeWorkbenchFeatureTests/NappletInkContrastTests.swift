import Foundation
@testable import RuntimeWorkbenchFeature
import Testing

private func relativeLuminance(_ rgb: Int) -> Double {
    func linearChannel(_ byte: Int) -> Double {
        let value = Double(byte) / 255
        return value <= 0.04045
            ? value / 12.92
            : pow((value + 0.055) / 1.055, 2.4)
    }

    return 0.2126 * linearChannel((rgb >> 16) & 0xFF)
        + 0.7152 * linearChannel((rgb >> 8) & 0xFF)
        + 0.0722 * linearChannel(rgb & 0xFF)
}

private func contrastRatio(_ foreground: Int, on background: Int) -> Double {
    let first = relativeLuminance(foreground)
    let second = relativeLuminance(background)
    return (max(first, second) + 0.05) / (min(first, second) + 0.05)
}

private struct ContrastRequirement {
    let name: String
    let foreground: NappletRGBPair
    let background: NappletRGBPair
    let minimum: Double
}

private let onPaperRequirements = [
    ContrastRequirement(
        name: "ink",
        foreground: NappletInkPalette.ink,
        background: NappletInkPalette.paper,
        minimum: 7
    ),
    ContrastRequirement(
        name: "inkSecondary",
        foreground: NappletInkPalette.inkSecondary,
        background: NappletInkPalette.paper,
        minimum: 4.5
    ),
    ContrastRequirement(
        name: "inkTertiary",
        foreground: NappletInkPalette.inkTertiary,
        background: NappletInkPalette.paper,
        minimum: 3
    ),
    ContrastRequirement(
        name: "accent",
        foreground: NappletInkPalette.accent,
        background: NappletInkPalette.paper,
        minimum: 4.5
    ),
    ContrastRequirement(
        name: "caution",
        foreground: NappletInkPalette.caution,
        background: NappletInkPalette.paper,
        minimum: 4.5
    ),
    ContrastRequirement(
        name: "refusal",
        foreground: NappletInkPalette.refusal,
        background: NappletInkPalette.paper,
        minimum: 4.5
    ),
]

@Test func productionInksClearTheirThresholdsOnProductionPaper() {
    for requirement in onPaperRequirements {
        let light = contrastRatio(
            requirement.foreground.light,
            on: requirement.background.light
        )
        let dark = contrastRatio(
            requirement.foreground.dark,
            on: requirement.background.dark
        )
        #expect(
            light >= requirement.minimum,
            "\(requirement.name) light: \(light)"
        )
        #expect(
            dark >= requirement.minimum,
            "\(requirement.name) dark: \(dark)"
        )
    }
}

@Test func productionActionLabelsClearBodyTextContrastOnAccent() {
    let light = contrastRatio(
        NappletInkPalette.onAccent.light,
        on: NappletInkPalette.accent.light
    )
    let dark = contrastRatio(
        NappletInkPalette.onAccent.dark,
        on: NappletInkPalette.accent.dark
    )

    #expect(light >= 4.5, "onAccent light: \(light)")
    #expect(dark >= 4.5, "onAccent dark: \(dark)")
}
