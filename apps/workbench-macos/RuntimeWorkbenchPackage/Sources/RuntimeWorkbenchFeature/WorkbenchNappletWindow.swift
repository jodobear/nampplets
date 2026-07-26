import SwiftUI

struct WorkbenchNappletWindow<Content: View>: View {
    let window: WorkbenchCanvasWindow
    let frame: CGRect
    let isSelected: Bool
    let isFreeform: Bool
    @ViewBuilder let content: () -> Content
    let select: () -> Void
    let move: (CGPoint, CGSize) -> Void
    let resize: (CGSize, CGSize) -> Void
    let close: () -> Void
    let commitLayout: () -> Void

    @State private var dragOrigin: CGPoint?
    @State private var resizeOrigin: CGSize?

    var body: some View {
        VStack(spacing: 0) {
            windowBar
            content()
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .clipped()
        }
        .background(.background)
        .clipShape(RoundedRectangle(cornerRadius: 10))
        .overlay {
            RoundedRectangle(cornerRadius: 10)
                .stroke(
                    isSelected
                        ? Color.accentColor
                        : Color.secondary.opacity(0.28),
                    lineWidth: isSelected ? 2 : 1
                )
                .allowsHitTesting(false)
        }
        .shadow(
            color: .black.opacity(isSelected ? 0.18 : 0.1),
            radius: isSelected ? 12 : 7,
            y: 3
        )
        .overlay(alignment: .bottomTrailing) {
            if isFreeform {
                resizeHandle
            }
        }
        .contentShape(Rectangle())
        .simultaneousGesture(
            TapGesture().onEnded {
                select()
            }
        )
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(window.title) napplet window")
        .accessibilityHint(
            isFreeform
                ? "Drag the title bar to move and the bottom-right handle to resize."
                : "Switch to Freeform layout to move or resize this window."
        )
        .accessibilityIdentifier("napplet-window-\(window.id.rawValue)")
    }

    // Napplets carry no chrome by default -- no icon, no title text, no
    // filled bar -- only the selection/hover border below. This strip stays
    // just tall enough to remain a drag handle in Freeform layout and to
    // host the close control; it renders no visible background of its own.
    private var windowBar: some View {
        HStack(spacing: 0) {
            Spacer()
            Button(action: close) {
                Image(systemName: "xmark.circle.fill")
                    .font(.system(size: 13))
            }
            .buttonStyle(.borderless)
            .foregroundStyle(.secondary.opacity(0.55))
            .accessibilityLabel("Close \(window.title)")
            .accessibilityHint(
                "Closes this window without uninstalling the napplet"
            )
        }
        .padding(.horizontal, 8)
        .padding(.top, 5)
        .frame(height: 22)
        .contentShape(Rectangle())
        .gesture(moveGesture)
        .accessibilityLabel("\(window.title) title bar")
    }

    private var moveGesture: some Gesture {
        DragGesture(
            minimumDistance: 2,
            coordinateSpace: .named("workbench-canvas")
        )
        .onChanged { value in
            guard isFreeform else {
                return
            }
            if dragOrigin == nil {
                dragOrigin = frame.origin
                select()
            }
            guard let dragOrigin else {
                return
            }
            move(dragOrigin, value.translation)
        }
        .onEnded { _ in
            guard dragOrigin != nil else {
                return
            }
            dragOrigin = nil
            commitLayout()
        }
    }

    private var resizeHandle: some View {
        Image(systemName: "arrow.down.right.and.arrow.up.left")
            .font(.caption2.weight(.semibold))
            .foregroundStyle(.secondary)
            .frame(width: 26, height: 26)
            .contentShape(Rectangle())
            .gesture(resizeGesture)
            .accessibilityLabel("Resize \(window.title)")
            .accessibilityHint("Drag to resize this napplet window")
    }

    private var resizeGesture: some Gesture {
        DragGesture(minimumDistance: 1)
            .onChanged { value in
                if resizeOrigin == nil {
                    resizeOrigin = frame.size
                    select()
                }
                guard let resizeOrigin else {
                    return
                }
                resize(resizeOrigin, value.translation)
            }
            .onEnded { _ in
                guard resizeOrigin != nil else {
                    return
                }
                resizeOrigin = nil
                commitLayout()
            }
    }
}
