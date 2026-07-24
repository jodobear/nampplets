import Foundation
import NMPNativeRuntime

struct NativeSettingsDocument: @unchecked Sendable {
    let request: NativeSettingsRequest
    let schema: [String: Any]
    let values: [String: Any]

    static func decode(_ request: NativeSettingsRequest) -> NativeSettingsDocument? {
        guard request.schemaJson.utf8.count <= 192 * 1_024,
              request.valuesJson.utf8.count <= 192 * 1_024,
              let schemaData = request.schemaJson.data(using: .utf8),
              let valuesData = request.valuesJson.data(using: .utf8),
              let schema = try? JSONSerialization.jsonObject(with: schemaData)
                as? [String: Any],
              let values = try? JSONSerialization.jsonObject(with: valuesData)
                as? [String: Any]
        else {
            return nil
        }
        return NativeSettingsDocument(request: request, schema: schema, values: values)
    }
}

enum SettingsPresentationError: LocalizedError {
    case invalidInteger
    case invalidNumber
    case invalidArray
    case invalidChoice

    var errorDescription: String? {
        switch self {
        case .invalidInteger: "Enter a whole number."
        case .invalidNumber: "Enter a finite number."
        case .invalidArray: "Enter a valid JSON array."
        case .invalidChoice: "Choose one of the available values."
        }
    }
}

func titleText(_ schema: [String: Any], fallback: String) -> String {
    (schema["title"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? fallback
}

func descriptionText(_ schema: [String: Any]) -> String? {
    for key in ["markdownDescription", "description", "deprecationMessage"] {
        if let value = schema[key] as? String, !value.isEmpty {
            return value
        }
    }
    return nil
}

func jsonEqual(_ left: Any, _ right: Any) -> Bool {
    guard JSONSerialization.isValidJSONObject([left]),
          JSONSerialization.isValidJSONObject([right]),
          let lhs = try? JSONSerialization.data(withJSONObject: [left], options: [.sortedKeys]),
          let rhs = try? JSONSerialization.data(withJSONObject: [right], options: [.sortedKeys])
    else {
        return false
    }
    return lhs == rhs
}

func jsonString(_ value: Any?) -> String? {
    guard let value,
          JSONSerialization.isValidJSONObject(value),
          let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    else {
        return nil
    }
    return String(data: data, encoding: .utf8)
}

extension Array {
    subscript(safe index: Index) -> Element? {
        indices.contains(index) ? self[index] : nil
    }
}

extension Optional {
    func isSomeAnd(_ predicate: (Wrapped) -> Bool) -> Bool {
        map(predicate) ?? false
    }
}
