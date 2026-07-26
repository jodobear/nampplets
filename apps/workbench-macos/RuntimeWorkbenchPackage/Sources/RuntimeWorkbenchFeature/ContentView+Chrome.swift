import NMPNativeRuntimeApple
import SwiftUI

extension ContentView {
    @ViewBuilder
    var platformBody: some View {
        #if os(iOS)
        if layout.mode == .fullWindow {
            WorkbenchFullWindowView(
                layout: $layout,
                rootID: fullWindowRootID,
                path: $fullWindowPath,
                onExit: exitFullWindow,
                windowContent: windowContent,
                topBars: { topStatusBars }
            )
        } else {
            NavigationStack {
                canvasBody
                    .navigationTitle("Napplets")
                    .navigationBarTitleDisplayMode(.inline)
                    .toolbar {
                        ToolbarItem(placement: .topBarLeading) {
                            accountMenu
                        }
                        ToolbarItemGroup(placement: .topBarTrailing) {
                            Button {
                                isCatalogSheetPresented = true
                            } label: {
                                Label("Add Napplet", systemImage: "plus")
                            }
                            .accessibilityIdentifier("add-napplet")
                            .accessibilityHint(
                                "Opens the network napplet catalog"
                            )

                            settingsToolbarButton

                            Button {
                                withAnimation(.easeInOut(duration: 0.18)) {
                                    isInspectorPresented.toggle()
                                }
                            } label: {
                                Label(
                                    isInspectorPresented ? "Hide Inspector" : "Show Inspector",
                                    systemImage: "sidebar.right"
                                )
                            }
                            .accessibilityIdentifier("toggle-napplet-inspector")

                            layoutMenu
                        }
                    }
            }
        }
        #else
        canvasBody
            .toolbar {
                macOSToolbar
            }
        #endif
    }

    @ViewBuilder
    var topStatusBars: some View {
        if let pendingWrite = pendingWrites.writes.first {
            PendingWriteApprovalBar(write: pendingWrite) { approve in
                pendingWrites.decide(
                    pendingWrite,
                    approve: approve,
                    profile: profile
                )
            }
        }
        if let receipt = receipts.receipts.last {
            ReceiptStatusBar(receipt: receipt)
        }
    }
}
