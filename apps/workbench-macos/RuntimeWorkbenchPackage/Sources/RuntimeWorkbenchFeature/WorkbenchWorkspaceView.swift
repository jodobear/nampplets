import SwiftUI

struct WorkbenchWorkspaceView<WindowContent: View>: View {
    @Binding var layout: WorkbenchLayoutModel
    let onLayoutChange: () -> Void
    let onClose: (WorkbenchCanvasWindow) -> Void
    var onAddNapplet: (() -> Void)?
    @ViewBuilder let windowContent: (WorkbenchCanvasWindow) -> WindowContent

    private let canvasPadding = 12.0
    private let tileSpacing = 12.0

    var body: some View {
        GeometryReader { proxy in
            ZStack(alignment: .topLeading) {
                canvasBackground

                if layout.windows.isEmpty {
                    emptyCanvas
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    ForEach(layout.windows) { window in
                        canvasWindow(
                            window,
                            canvasSize: proxy.size
                        )
                    }
                }
            }
            .coordinateSpace(name: "workbench-canvas")
            .contentShape(Rectangle())
            .onTapGesture {
                var next = layout
                next.select(nil)
                guard next != layout else {
                    return
                }
                layout = next
                onLayoutChange()
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Napplet canvas")
    }

    /// The first thing a new person sees, so it says what this app is for
    /// rather than reporting that a data structure is empty.
    private var emptyCanvas: some View {
        VStack(spacing: NappletMetrics.comfortable) {
            Image(systemName: "square.grid.2x2")
                .font(.system(size: 34, weight: .light))
                .foregroundStyle(.tertiary)
                .accessibilityHidden(true)

            VStack(spacing: NappletMetrics.tight) {
                Text("Nothing open yet")
                    .font(.title3.weight(.semibold))
                Text(
                    "Napplets are small apps you can add and arrange here. "
                        + "Everything they're allowed to do is up to you."
                )
                .font(.callout)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 380)
                .fixedSize(horizontal: false, vertical: true)
            }

            if let onAddNapplet {
                Button("Browse Napplets", action: onAddNapplet)
                    .buttonStyle(.borderedProminent)
                    .controlSize(.large)
                    .accessibilityIdentifier("empty-canvas-add-napplet")
            }
        }
        .padding(NappletMetrics.generous)
        .accessibilityElement(children: .contain)
    }

    private var canvasBackground: some View {
        Rectangle()
            .fill(.background)
            .overlay {
                Canvas { context, size in
                    let spacing = 24.0
                    var path = Path()
                    var x = spacing
                    while x < size.width {
                        var y = spacing
                        while y < size.height {
                            path.addEllipse(
                                in: CGRect(
                                    x: x,
                                    y: y,
                                    width: 1.25,
                                    height: 1.25
                                )
                            )
                            y += spacing
                        }
                        x += spacing
                    }
                    context.fill(
                        path,
                        with: .color(.secondary.opacity(0.16))
                    )
                }
                .allowsHitTesting(false)
            }
    }

    private func canvasWindow(
        _ window: WorkbenchCanvasWindow,
        canvasSize: CGSize
    ) -> some View {
        let frame = renderedFrame(
            for: window,
            canvasSize: canvasSize
        )
        return WorkbenchNappletWindow(
            window: window,
            frame: frame,
            isSelected: layout.snapshot.selectedWindowID == window.id,
            isFreeform: layout.mode == .freeform,
            content: { windowContent(window) },
            select: {
                var next = layout
                next.bringToFront(window.id)
                guard next != layout else {
                    return
                }
                layout = next
                onLayoutChange()
            },
            move: { origin, translation in
                guard layout.mode == .freeform else {
                    return
                }
                var next = layout
                next.moveWindow(
                    id: window.id,
                    x: origin.x + translation.width,
                    y: origin.y + translation.height,
                    canvasSize: canvasSize
                )
                layout = next
            },
            resize: { origin, translation in
                guard layout.mode == .freeform else {
                    return
                }
                var next = layout
                next.resizeWindow(
                    id: window.id,
                    width: origin.width + translation.width,
                    height: origin.height + translation.height,
                    canvasSize: canvasSize
                )
                layout = next
            },
            close: {
                onClose(window)
            },
            commitLayout: onLayoutChange
        )
        .frame(width: frame.width, height: frame.height)
        .position(
            x: frame.minX + frame.width / 2,
            y: frame.minY + frame.height / 2
        )
        .zIndex(Double(window.stackingOrder))
    }

    private func renderedFrame(
        for window: WorkbenchCanvasWindow,
        canvasSize: CGSize
    ) -> CGRect {
        switch layout.mode {
        case .freeform:
            let fitted = window.frame.fitted(to: canvasSize)
            return CGRect(
                x: fitted.x,
                y: fitted.y,
                width: fitted.width,
                height: fitted.height
            )
        case .tiling, .fullWindow:
            // `.fullWindow` is presented by a dedicated chrome-less screen on
            // iOS (see WorkbenchFullWindowView); this tiling fallback only
            // applies if the freeform/tiling canvas ever renders a workspace
            // synced from a device that saved that mode.
            let ordered = layout.windows
            guard
                let index = ordered.firstIndex(where: { $0.id == window.id })
            else {
                return .zero
            }
            let count = ordered.count
            let columns = max(Int(ceil(sqrt(Double(count)))), 1)
            let rows = max(Int(ceil(Double(count) / Double(columns))), 1)
            let usableWidth = max(
                canvasSize.width
                    - canvasPadding * 2
                    - tileSpacing * Double(columns - 1),
                WorkbenchWindowFrame.minimumWidth
            )
            let usableHeight = max(
                canvasSize.height
                    - canvasPadding * 2
                    - tileSpacing * Double(rows - 1),
                WorkbenchWindowFrame.minimumHeight
            )
            let width = usableWidth / Double(columns)
            let height = usableHeight / Double(rows)
            let column = index % columns
            let row = index / columns
            return CGRect(
                x: canvasPadding + Double(column) * (width + tileSpacing),
                y: canvasPadding + Double(row) * (height + tileSpacing),
                width: width,
                height: height
            )
        }
    }
}
