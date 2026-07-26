import Foundation
import NMPNativeRuntime

final class RecordingRuntimeObserver: RuntimeObserver, @unchecked Sendable {
    private let condition = NSCondition()
    private var revision: UInt64?

    var latestRevision: UInt64? {
        condition.lock()
        defer { condition.unlock() }
        return revision
    }

    func update(frame: RuntimeObservationFrame) {
        condition.lock()
        revision = frame.snapshot.revision
        condition.broadcast()
        condition.unlock()
    }

    func waitForInitialFrame(timeout: TimeInterval) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        condition.lock()
        defer { condition.unlock() }
        while revision == nil {
            guard condition.wait(until: deadline) else {
                return false
            }
        }
        return true
    }
}

final class ResponseRuntimeObserver: RuntimeObserver, @unchecked Sendable {
    private let condition = NSCondition()
    private var responses: [String] = []

    func update(frame: RuntimeObservationFrame) {
        let delivered = frame.events.compactMap(\.responseJson)
        guard !delivered.isEmpty else { return }
        condition.lock()
        responses.append(contentsOf: delivered)
        condition.broadcast()
        condition.unlock()
    }

    func waitForResponse(
        type: String,
        id: String?,
        timeout: TimeInterval
    ) -> [String: Any]? {
        let deadline = Date().addingTimeInterval(timeout)
        condition.lock()
        defer { condition.unlock() }
        while true {
            if let response = responses.compactMap(decode).first(where: {
                $0["type"] as? String == type
                    && (id == nil || $0["id"] as? String == id)
            }) {
                return response
            }
            guard condition.wait(until: deadline) else {
                return nil
            }
        }
    }

    private func decode(_ raw: String) -> [String: Any]? {
        guard
            let data = raw.data(using: .utf8),
            let value = try? JSONSerialization.jsonObject(with: data)
        else {
            return nil
        }
        return value as? [String: Any]
    }
}
