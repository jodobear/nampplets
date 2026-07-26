#if os(iOS)
import SwiftUI

struct WorkbenchSettingsSheet: View {
    @Environment(\.dismiss) private var dismiss

    let snapshot: WorkbenchSettingsSnapshot?
    let performAction: WorkbenchProfileActionHandler

    var body: some View {
        WorkbenchSettingsView(
            snapshot: snapshot,
            performAction: performAction
        )
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Close") {
                    dismiss()
                }
            }
        }
    }
}
#endif
