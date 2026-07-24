import Foundation
import NMPNativeRuntimeApple
import SwiftUI

/// Main-actor presentation model for the Rust-owned pending-write projection.
/// It retains no draft authority: approval forwards only the opaque operation
/// id back to the native profile.
@MainActor
final class RuntimeWorkbenchPendingWriteModel: ObservableObject {
    @Published private(set) var writes: [NativeRuntimePendingWrite] = []

    private var observation: NativeRuntimePendingWriteObservation?

    init(profile: WorkbenchRuntimeProfile?) {
        guard let profile else { return }
        do {
            observation = try profile.native.observePendingWrites {
                [weak self] update in
                Task { @MainActor [weak self] in
                    self?.receive(update)
                }
            }
        } catch {
            writes = []
        }
    }

    func decide(
        _ write: NativeRuntimePendingWrite,
        approve: Bool,
        profile: WorkbenchRuntimeProfile?
    ) {
        profile?.native.decideProviderWrite(
            operationID: write.id,
            approve: approve
        )
    }

    private func receive(_ update: NativeRuntimePendingWriteUpdate) {
        switch update {
        case let .authoritative(projection),
             let .next(projection, _, _):
            writes = projection.writes
        }
    }

    deinit {
        observation?.cancel()
    }
}

/// Keeps the latest bounded canonical receipt projection visible after the
/// originating napplet/session changes state or closes.
@MainActor
final class RuntimeWorkbenchReceiptModel: ObservableObject {
    @Published private(set) var receipts: [NativeRuntimeReceipt] = []

    private var observation: NativeRuntimeReceiptObservation?

    init(profile: WorkbenchRuntimeProfile?) {
        guard let profile else { return }
        do {
            observation = try profile.native.observeReceipts {
                [weak self] update in
                Task { @MainActor [weak self] in
                    self?.receive(update)
                }
            }
        } catch {
            receipts = []
        }
    }

    private func receive(_ update: NativeRuntimeReceiptUpdate) {
        switch update {
        case let .authoritative(projection),
             let .next(projection, _, _):
            receipts = projection.receipts
        }
    }

    deinit {
        observation?.cancel()
    }
}

struct PendingWriteApprovalBar: View {
    let write: NativeRuntimePendingWrite
    let onDecision: (Bool) -> Void

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "signature")
                .foregroundStyle(.orange)
            VStack(alignment: .leading, spacing: 2) {
                Text("NAP-OUTBOX approval required")
                    .font(.headline)
                Text("\(write.account) · \(write.approvalID)")
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                Text(write.draftJSON)
                    .font(.caption2.monospaced())
                    .lineLimit(2)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 12)
            Button("Reject") {
                onDecision(false)
            }
            .buttonStyle(.bordered)
            Button("Approve") {
                onDecision(true)
            }
            .buttonStyle(.borderedProminent)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 9)
        .background(.regularMaterial)
        .overlay(alignment: .bottom) {
            Rectangle()
                .fill(.orange.opacity(0.55))
                .frame(height: 1)
        }
        .accessibilityIdentifier("nap-outbox-pending-approval")
    }
}

struct ReceiptStatusBar: View {
    let receipt: NativeRuntimeReceipt

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: symbol)
                .foregroundStyle(color)
            Text("NMP receipt")
                .font(.caption.weight(.semibold))
            Text(receipt.id)
                .font(.caption2.monospaced())
                .lineLimit(1)
            Text(receipt.delivery)
                .font(.caption2.monospaced())
                .foregroundStyle(.secondary)
            Spacer()
            if let latestStateJSON = receipt.latestStateJSON {
                Text(latestStateJSON)
                    .font(.caption2.monospaced())
                    .lineLimit(1)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 6)
        .background(.regularMaterial)
        .accessibilityIdentifier("nap-outbox-receipt-status")
    }

    private var symbol: String {
        receipt.delivery.contains("pending") ? "clock" : "checkmark.seal.fill"
    }

    private var color: Color {
        receipt.delivery.contains("pending") ? .orange : .green
    }
}
