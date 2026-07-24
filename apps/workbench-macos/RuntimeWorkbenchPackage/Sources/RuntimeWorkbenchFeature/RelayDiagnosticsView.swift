import Observation
import SwiftUI

@MainActor
@Observable
final class RelayDiagnosticsViewModel {
    private(set) var snapshot: InspectorRelayDiagnosticsSnapshot = .notObserving

    private let source: any RelayDiagnosticsSource
    private var subscription: (any RelayDiagnosticsSubscription)?

    init(source: any RelayDiagnosticsSource) {
        self.source = source
    }

    func start() {
        guard subscription == nil else {
            return
        }
        subscription = source.subscribe { [weak self] snapshot in
            self?.snapshot = snapshot
        }
    }

    func stop() {
        subscription?.cancel()
        subscription = nil
        snapshot = .notObserving
    }
}

/// The Relays & Subscriptions Inspector tab. Opens the real NMP diagnostics
/// observation for exactly this view's on-screen lifetime — appearing opens
/// it, disappearing (tab switch, panel close, window close) withdraws it.
struct RelayDiagnosticsInspectorView: View {
    @State private var model: RelayDiagnosticsViewModel

    init(source: any RelayDiagnosticsSource) {
        _model = State(initialValue: RelayDiagnosticsViewModel(source: source))
    }

    var body: some View {
        Group {
            if !model.snapshot.observing {
                ContentUnavailableView(
                    "Not observing relays",
                    systemImage: "antenna.radiowaves.left.and.right.slash",
                    description: Text("Waiting for the runtime diagnostics observation to open.")
                )
            } else if model.snapshot.relays.isEmpty {
                ContentUnavailableView(
                    "No relays planned",
                    systemImage: "antenna.radiowaves.left.and.right",
                    description: Text("The engine has not planned a relay session yet.")
                )
            } else {
                List {
                    ForEach(model.snapshot.relays) { relay in
                        RelayDiagnosticsRow(relay: relay)
                    }
                    if model.snapshot.omittedRelays > 0 {
                        Text("\(model.snapshot.omittedRelays) relays omitted by the runtime projection")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                .listStyle(.sidebar)
            }
        }
        .onAppear {
            model.start()
        }
        .onDisappear {
            model.stop()
        }
        .accessibilityIdentifier("inspector-relay-diagnostics")
    }
}

private struct RelayDiagnosticsRow: View {
    let relay: InspectorRelayDiagnostics

    var body: some View {
        DisclosureGroup {
            VStack(alignment: .leading, spacing: 8) {
                LabeledContent("Access", value: relay.access.title)
                LabeledContent("Authors served", value: relay.authorsServed.formatted())

                if !relay.subscriptions.isEmpty {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Active subscriptions")
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(.secondary)
                        ForEach(relay.subscriptions) { subscription in
                            SubscriptionRow(subscription: subscription)
                        }
                        if relay.omittedSubscriptions > 0 {
                            Text("\(relay.omittedSubscriptions) subscriptions omitted")
                                .font(.caption2)
                                .foregroundStyle(.secondary)
                        }
                    }
                }

                if !relay.eventsByKind.isEmpty {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Events by kind")
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(.secondary)
                        ForEach(relay.eventsByKind) { kindCount in
                            LabeledContent(
                                "Kind \(kindCount.kind)",
                                value: kindCount.events.formatted()
                            )
                            .font(.caption.monospaced())
                        }
                    }
                }

                if let nip11Error = relay.nip11LastError {
                    Label(nip11Error, systemImage: "exclamationmark.triangle")
                        .font(.caption)
                        .foregroundStyle(.orange)
                }
            }
            .padding(.top, 4)
        } label: {
            VStack(alignment: .leading, spacing: 3) {
                Text(relay.relay)
                    .font(.subheadline.weight(.semibold))
                    .textSelection(.enabled)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Text(
                    "\(relay.wireSubscriptionCount) wire subscription"
                        + (relay.wireSubscriptionCount == 1 ? "" : "s")
                )
                .font(.caption)
                .foregroundStyle(.secondary)
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(
            "\(relay.relay), \(relay.wireSubscriptionCount) wire subscriptions"
        )
    }
}

private struct SubscriptionRow: View {
    let subscription: InspectorRelaySubscription

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(subscription.filter)
                .font(.caption.monospaced())
                .textSelection(.enabled)
                .lineLimit(2)
                .truncationMode(.tail)
            Text(coverageDescription)
                .font(.caption2)
                .foregroundStyle(.secondary)
        }
    }

    private var coverageDescription: String {
        guard let coverage = subscription.coverage else {
            return "Coverage unproven"
        }
        let from = Date(timeIntervalSince1970: TimeInterval(coverage.fromSeconds))
        let through = Date(timeIntervalSince1970: TimeInterval(coverage.throughSeconds))
        return "Covered \(from.formatted(.relative(presentation: .named))) – "
            + through.formatted(.relative(presentation: .named))
    }
}
