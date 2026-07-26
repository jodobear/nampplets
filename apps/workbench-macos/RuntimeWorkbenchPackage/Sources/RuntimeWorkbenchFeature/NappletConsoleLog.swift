import SwiftUI

/// One mirrored `console.log`/`console.warn`/`console.error`/uncaught-error
/// line from a napplet's own sandboxed JavaScript, captured by the trusted
/// shell and reported natively. Diagnostic only -- this never crosses into
/// Rust and carries no NAP domain authority.
public struct NappletConsoleEntry: Identifiable, Equatable, Sendable {
    public let id: Int
    public let level: String
    public let message: String

    public init(id: Int, level: String, message: String) {
        self.id = id
        self.level = level
        self.message = message
    }
}

/// Bounded per-window ring buffer for `NappletConsoleEntry` values, keyed by
/// exact-build identity. Each window's log is capped independently so one
/// chatty napplet cannot crowd out another's history.
struct NappletConsoleLog {
    private static let maximumEntriesPerWindow = 500

    private var entriesByIdentity: [WorkbenchExactBuildIdentity: [NappletConsoleEntry]] = [:]
    private var nextID = 0

    func entries(for identity: WorkbenchExactBuildIdentity?) -> [NappletConsoleEntry] {
        guard let identity else { return [] }
        return entriesByIdentity[identity] ?? []
    }

    mutating func append(level: String, message: String, for identity: WorkbenchExactBuildIdentity) {
        nextID += 1
        var entries = entriesByIdentity[identity] ?? []
        entries.append(NappletConsoleEntry(id: nextID, level: level, message: message))
        if entries.count > Self.maximumEntriesPerWindow {
            entries.removeFirst(entries.count - Self.maximumEntriesPerWindow)
        }
        entriesByIdentity[identity] = entries
    }

    mutating func clear(for identity: WorkbenchExactBuildIdentity) {
        entriesByIdentity[identity] = nil
    }
}

struct NappletConsoleTabView: View {
    let entries: [NappletConsoleEntry]
    let onClear: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if entries.isEmpty {
                ContentUnavailableView(
                    "No console output yet",
                    systemImage: "terminal",
                    description: Text(
                        "console.log, console.warn, console.error, and uncaught "
                            + "napplet errors appear here as they happen."
                    )
                )
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 4) {
                            ForEach(entries) { entry in
                                consoleRow(entry)
                                    .id(entry.id)
                            }
                        }
                    }
                    .onChange(of: entries.last?.id) { _, lastID in
                        guard let lastID else { return }
                        withAnimation(.easeOut(duration: 0.15)) {
                            proxy.scrollTo(lastID, anchor: .bottom)
                        }
                    }
                }
                Button("Clear Console", role: .destructive, action: onClear)
                    .buttonStyle(.borderless)
            }
        }
        .accessibilityIdentifier("napplet-console")
    }

    private func consoleRow(_ entry: NappletConsoleEntry) -> some View {
        HStack(alignment: .top, spacing: 6) {
            Image(systemName: symbolName(for: entry.level))
                .foregroundStyle(color(for: entry.level))
                .font(.caption)
                .frame(width: 14)
            Text(entry.message)
                .font(.system(.caption, design: .monospaced))
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func symbolName(for level: String) -> String {
        switch level {
        case "error": "xmark.octagon.fill"
        case "warn": "exclamationmark.triangle.fill"
        default: "chevron.right"
        }
    }

    private func color(for level: String) -> Color {
        switch level {
        case "error": .red
        case "warn": .orange
        default: .secondary
        }
    }
}
