import SwiftUI

struct WorkbenchWorkspaceView<SlotContent: View>: View {
    @Binding var layout: WorkbenchLayoutModel
    var focusedRole: FocusState<WorkbenchSlotRole?>.Binding
    let onLayoutChange: () -> Void
    @ViewBuilder let slotContent: (WorkbenchSlotRole) -> SlotContent

    private let topRoles: [WorkbenchSlotRole] = [.feed, .detail, .tool]

    var body: some View {
        Group {
            if layout.isVisible(.composer) {
                VSplitView {
                    topRow
                    slot(.composer)
                }
            } else {
                topRow
            }
        }
        .onPreferenceChange(WorkbenchSlotSizePreferenceKey.self) { sizes in
            var changed = false
            var next = layout
            for (role, size) in sizes {
                changed = next.recordRenderedSize(
                    role: role,
                    width: size.width,
                    height: size.height
                ) || changed
            }
            if changed {
                layout = next
                onLayoutChange()
            }
        }
    }

    @ViewBuilder
    private var topRow: some View {
        let visibleTopRoles = topRoles.filter(layout.isVisible)
        if visibleTopRoles.isEmpty {
            ContentUnavailableView {
                Label("No top slots are visible", systemImage: "rectangle.slash")
            } description: {
                Text("Show the Feed, Detail, or Tool slot from the Layout menu.")
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            HSplitView {
                ForEach(visibleTopRoles, id: \.self) { role in
                    slot(role)
                }
            }
        }
    }

    private func slot(_ role: WorkbenchSlotRole) -> some View {
        let constraints = role.constraints
        let size = layout.size(for: role)
        return VStack(spacing: 0) {
            HStack(spacing: 8) {
                Label(role.title, systemImage: role.systemImage)
                    .font(.headline)
                if let component = layout.component(in: role) {
                    Text(component.title)
                        .font(.caption.weight(.medium))
                        .padding(.horizontal, 7)
                        .padding(.vertical, 3)
                        .background(.tint.opacity(0.12), in: Capsule())
                        .accessibilityLabel("\(component.title) assigned")
                }
                Spacer()
                slotMenu(role)
            }
            .padding(.horizontal, 10)
            .frame(height: 36)
            .background(.bar)
            .contentShape(Rectangle())
            .onTapGesture {
                focus(role)
            }

            slotContent(role)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .background(.background)
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .overlay {
            RoundedRectangle(cornerRadius: 8)
                .stroke(
                    focusedRole.wrappedValue == role
                        ? Color.accentColor
                        : Color.secondary.opacity(0.22),
                    lineWidth: focusedRole.wrappedValue == role ? 2 : 1
                )
        }
        .contentShape(Rectangle())
        .focusable()
        .focused(focusedRole, equals: role)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(role.title) workspace slot")
        .accessibilityHint(
            "Use the slot actions menu to focus, hide, or move Good Morning here."
        )
        .background {
            GeometryReader { proxy in
                Color.clear.preference(
                    key: WorkbenchSlotSizePreferenceKey.self,
                    value: [role: proxy.size]
                )
            }
        }
        .frame(
            minWidth: constraints.minimumWidth,
            idealWidth: size.width,
            maxWidth: constraints.maximumWidth,
            minHeight: constraints.minimumHeight,
            idealHeight: size.height,
            maxHeight: constraints.maximumHeight
        )
    }

    private func slotMenu(_ role: WorkbenchSlotRole) -> some View {
        Menu {
            Button {
                var next = layout
                next.move(.goodMorning, to: role)
                layout = next
                focusedRole.wrappedValue = role
                onLayoutChange()
            } label: {
                Label("Move Good Morning Here", systemImage: "arrow.right.square")
            }

            Divider()

            Button {
                focus(role)
            } label: {
                Label("Focus \(role.title)", systemImage: "scope")
            }

            Button(role: .destructive) {
                var next = layout
                next.setVisible(false, role: role)
                layout = next
                focusedRole.wrappedValue = next.snapshot.focusedRole
                onLayoutChange()
            } label: {
                Label("Hide \(role.title)", systemImage: "eye.slash")
            }
        } label: {
            Image(systemName: "ellipsis.circle")
                .imageScale(.large)
                .frame(width: 28, height: 28)
        }
        .menuStyle(.borderlessButton)
        .accessibilityLabel("\(role.title) slot actions")
    }

    private func focus(_ role: WorkbenchSlotRole) {
        var next = layout
        next.focus(role)
        layout = next
        focusedRole.wrappedValue = role
        onLayoutChange()
    }
}

private struct WorkbenchSlotSizePreferenceKey: PreferenceKey {
    static let defaultValue: [WorkbenchSlotRole: CGSize] = [:]

    static func reduce(
        value: inout [WorkbenchSlotRole: CGSize],
        nextValue: () -> [WorkbenchSlotRole: CGSize]
    ) {
        value.merge(nextValue(), uniquingKeysWith: { _, next in next })
    }
}
