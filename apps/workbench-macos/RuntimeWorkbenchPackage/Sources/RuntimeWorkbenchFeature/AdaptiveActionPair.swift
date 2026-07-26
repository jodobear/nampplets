import SwiftUI

/// Keeps paired actions readable in narrow sheets and at large text sizes.
struct AdaptiveActionPair<
    Leading: View,
    Trailing: View
>: View {
    @ViewBuilder let leading: () -> Leading
    @ViewBuilder let trailing: () -> Trailing

    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: NappletMetrics.snug) {
                Spacer(minLength: 0)
                leading()
                    .fixedSize(horizontal: true, vertical: true)
                trailing()
                    .fixedSize(horizontal: true, vertical: true)
            }

            VStack(alignment: .trailing, spacing: NappletMetrics.snug) {
                leading()
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .trailing)
                trailing()
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .trailing)
            }
        }
    }
}
