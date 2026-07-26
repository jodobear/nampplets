import SwiftUI

struct ActivitySheetPresentation: Identifiable {
    enum Content {
        case admitted(
            source: RuntimeWorkbenchActivitySource,
            scope: ActivityExactBuildScope,
            title: String?
        )
        case unavailable(reason: String)
    }

    let id = UUID()
    let content: Content

    static func admitted(
        source: RuntimeWorkbenchActivitySource,
        scope: ActivityExactBuildScope,
        title: String? = nil
    ) -> Self {
        Self(content: .admitted(source: source, scope: scope, title: title))
    }

    static func unavailable(reason: String) -> Self {
        Self(content: .unavailable(reason: reason))
    }
}

/// Presents the exact-build activity drawer, or a truthful unavailable
/// fallback when no activity source or scope was admitted.
struct ActivitySheetHost: View {
    let presentation: ActivitySheetPresentation

    @ViewBuilder
    var body: some View {
        switch presentation.content {
        case let .admitted(source, scope, title):
            ActivityDrawer(
                source: source,
                scope: scope,
                nappletTitle: title
            )
        case let .unavailable(reason):
            let presentation = WorkbenchUnavailablePresentation.activity(
                detail: reason
            )
            NavigationStack {
                VStack(spacing: NappletMetrics.comfortable) {
                    ContentUnavailableView(
                        presentation.title,
                        systemImage: presentation.symbol,
                        description: Text(presentation.message)
                    )
                    NappletEvidence {
                        NappletFieldGrid(fields: presentation.evidenceFields)
                    }
                    .frame(maxWidth: NappletMetrics.measure)
                }
                .navigationTitle("Recent Activity")
                #if os(macOS)
                .frame(minWidth: 620, minHeight: 420)
                #endif
            }
        }
    }
}

/// Presents the permission review sheet, or a truthful unavailable fallback
/// when no permission manager was admitted.
struct PermissionSheetHost: View {
    let manager: (any PermissionReviewManaging)?
    let error: String?

    var body: some View {
        if let manager {
            PermissionReviewSheet(manager: manager)
        } else {
            let presentation = WorkbenchUnavailablePresentation.permission(
                detail: error ?? "No permission manager was admitted."
            )
            NavigationStack {
                VStack(spacing: NappletMetrics.comfortable) {
                    ContentUnavailableView(
                        presentation.title,
                        systemImage: presentation.symbol,
                        description: Text(presentation.message)
                    )
                    NappletEvidence {
                        NappletFieldGrid(fields: presentation.evidenceFields)
                    }
                    .frame(maxWidth: NappletMetrics.measure)
                }
                .navigationTitle("Review Permissions")
                #if os(macOS)
                .frame(minWidth: 620, minHeight: 420)
                #endif
            }
        }
    }
}

/// Presents the settings sheet, or a truthful unavailable fallback when no
/// settings snapshot was captured.
struct SettingsSheetHost: View {
    let snapshot: WorkbenchSettingsSnapshot?
    let openDestination: (WorkbenchSettingsDestination) -> Void
    let performAction: WorkbenchProfileActionHandler

    var body: some View {
        if let snapshot {
            WorkbenchSettingsSheet(
                snapshot: snapshot,
                openDestination: openDestination,
                performAction: performAction
            )
        } else {
            NavigationStack {
                ContentUnavailableView(
                    "Settings unavailable",
                    systemImage: "gearshape.fill",
                    description: Text(
                        "Preferences could not be displayed."
                    )
                )
                .navigationTitle("Preferences")
                #if os(macOS)
                .frame(minWidth: 620, minHeight: 420)
                #endif
            }
        }
    }
}
