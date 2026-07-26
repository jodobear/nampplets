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
                            .accessibilityHint("Opens the network napplet catalog")

                            workspaceActionsMenu

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
        VStack(spacing: 0) {
            workspaceControlStrip
            canvasBody
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

    private var workspaceControlStrip: some View {
        HStack(spacing: 10) {
            accountMenu

            Text("Napplets")
                .font(.title3.weight(.semibold))
            Spacer()

            Button {
                isCatalogSheetPresented = true
            } label: {
                Label("Add Napplet", systemImage: "plus")
            }
            .buttonStyle(.borderedProminent)
            .keyboardShortcut("n", modifiers: [.command])
            .accessibilityIdentifier("add-napplet")
            .accessibilityHint("Opens the network napplet catalog")

            workspaceActionsMenu

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
            .labelStyle(.iconOnly)
            .buttonStyle(.borderless)
            .keyboardShortcut("i", modifiers: [.command, .option])
            .accessibilityIdentifier("toggle-napplet-inspector")

            layoutMenu
        }
        .padding(.horizontal, 14)
        .frame(height: 50)
        .background(.bar)
    }

    @MainActor
    func setLayoutMode(_ mode: WorkbenchLayoutMode) {
        mutateLayout { $0.setMode(mode) }
        if mode == .fullWindow {
            fullWindowRootID = layout.selectedWindow?.id
            fullWindowPath = []
        } else {
            fullWindowRootID = nil
            fullWindowPath = []
        }
    }

    @MainActor
    private func exitFullWindow() {
        setLayoutMode(.freeform)
    }
}
