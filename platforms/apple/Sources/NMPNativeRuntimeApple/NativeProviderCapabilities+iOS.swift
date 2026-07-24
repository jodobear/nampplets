#if os(iOS)
import Foundation
import NMPNativeRuntime
import SwiftUI
import UIKit

@MainActor
private func keyWindow() -> UIWindow? {
    UIApplication.shared.connectedScenes
        .compactMap { $0 as? UIWindowScene }
        .flatMap(\.windows)
        .first(where: \.isKeyWindow)
}

private final class TraitObserverView: UIView {
    var onChange: (() -> Void)?

    override func traitCollectionDidChange(_ previousTraitCollection: UITraitCollection?) {
        super.traitCollectionDidChange(previousTraitCollection)
        guard let previousTraitCollection,
              previousTraitCollection.userInterfaceStyle != traitCollection.userInterfaceStyle
        else { return }
        onChange?()
    }
}

/// Event-driven projection of UIKit appearance facts. The callback reports
/// raw OS traits only; Rust maps them to the pinned NAP-THEME schema.
final class IOSAppearanceSource: NSObject, NativeAppearanceSource, @unchecked Sendable {
    private let lock = NSLock()
    private var snapshot: NativeAppearanceSnapshot
    private weak var controller: RuntimeController?
    private var isClosed = false
    private var refreshPending = false
    private var observerView: TraitObserverView?
    private var contrastObserver: NSObjectProtocol?
    private var transparencyObserver: NSObjectProtocol?

    override init() {
        snapshot = Self.captureSynchronously()
        super.init()
    }

    func current() -> NativeAppearanceSnapshot? {
        lock.lock()
        defer { lock.unlock() }
        return isClosed ? nil : snapshot
    }

    func bind(controller: RuntimeController) {
        lock.lock()
        guard !isClosed else {
            lock.unlock()
            return
        }
        self.controller = controller
        lock.unlock()
        DispatchQueue.main.async { [weak self] in
            self?.installObservers()
        }
    }

    func close() {
        lock.lock()
        guard !isClosed else {
            lock.unlock()
            return
        }
        isClosed = true
        controller = nil
        lock.unlock()
        DispatchQueue.main.async { [weak self] in
            self?.removeObservers()
        }
    }

    @MainActor
    private func installObservers() {
        guard observerView == nil else { return }
        guard let window = keyWindow() else { return }
        let view = TraitObserverView(frame: .zero)
        view.isHidden = true
        view.onChange = { [weak self] in
            self?.scheduleRefresh()
        }
        window.addSubview(view)
        observerView = view
        contrastObserver = NotificationCenter.default.addObserver(
            forName: UIAccessibility.darkerSystemColorsStatusDidChangeNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.scheduleRefresh()
        }
        transparencyObserver = NotificationCenter.default.addObserver(
            forName: UIAccessibility.reduceTransparencyStatusDidChangeNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.scheduleRefresh()
        }
    }

    @MainActor
    private func removeObservers() {
        observerView?.removeFromSuperview()
        observerView = nil
        if let contrastObserver {
            NotificationCenter.default.removeObserver(contrastObserver)
            self.contrastObserver = nil
        }
        if let transparencyObserver {
            NotificationCenter.default.removeObserver(transparencyObserver)
            self.transparencyObserver = nil
        }
    }

    private func scheduleRefresh() {
        lock.lock()
        guard !isClosed, !refreshPending else {
            lock.unlock()
            return
        }
        refreshPending = true
        lock.unlock()
        DispatchQueue.main.async { [weak self] in
            self?.publishCurrentAppearance()
        }
    }

    @MainActor
    private func publishCurrentAppearance() {
        let next = Self.captureOnMainActor()
        lock.lock()
        refreshPending = false
        guard !isClosed else {
            lock.unlock()
            return
        }
        let changed = next != snapshot
        snapshot = next
        let controller = controller
        lock.unlock()
        if changed {
            _ = controller?.updateAppearance(appearance: next)
        }
    }

    private static func captureSynchronously() -> NativeAppearanceSnapshot {
        if Thread.isMainThread {
            return MainActor.assumeIsolated { captureOnMainActor() }
        }
        return DispatchQueue.main.sync {
            MainActor.assumeIsolated { captureOnMainActor() }
        }
    }

    @MainActor
    private static func captureOnMainActor() -> NativeAppearanceSnapshot {
        let style = keyWindow()?.traitCollection.userInterfaceStyle ?? .light
        let dark = style == .dark
        let accent = UIColor.systemBlue.resolvedColor(
            with: UITraitCollection(userInterfaceStyle: dark ? .dark : .light)
        )
        var red: CGFloat = 0
        var green: CGFloat = 0
        var blue: CGFloat = 0
        var alpha: CGFloat = 0
        accent.getRed(&red, green: &green, blue: &blue, alpha: &alpha)
        return NativeAppearanceSnapshot(
            dark: dark,
            increasedContrast: UIAccessibility.isDarkerSystemColorsEnabled,
            reducedTransparency: UIAccessibility.isReduceTransparencyEnabled,
            accentRed: component(red),
            accentGreen: component(green),
            accentBlue: component(blue)
        )
    }

    private static func component(_ value: CGFloat) -> UInt8 {
        UInt8(clamping: Int((min(max(value, 0), 1) * 255).rounded()))
    }
}

private struct SettingsField: Identifiable {
    enum Kind {
        case string(secret: Bool)
        case integer
        case number
        case boolean
        case array
        case enumeration(choices: [Any], labels: [String])
    }

    let id = UUID()
    let path: [String]
    let key: String
    let label: String
    let description: String?
    let kind: Kind
    let initiallyPresent: Bool
}

private final class SettingsNode: Identifiable {
    enum Body {
        case field(SettingsField)
        case group(title: String, description: String?, children: [SettingsNode])
    }

    let id = UUID()
    let body: Body

    init(_ body: Body) {
        self.body = body
    }
}

private func buildNodes(
    schema: [String: Any],
    current: [String: Any],
    path: [String],
    requestedSection: String?
) -> [SettingsNode] {
    let properties = schema["properties"] as? [String: Any] ?? [:]
    let ordered = properties.compactMap { key, value -> (String, [String: Any])? in
        guard let value = value as? [String: Any] else { return nil }
        return (key, value)
    }.sorted {
        let left = ($0.1["x-napplet-order"] as? NSNumber)?.doubleValue ?? .greatestFiniteMagnitude
        let right = ($1.1["x-napplet-order"] as? NSNumber)?.doubleValue ?? .greatestFiniteMagnitude
        return left == right ? $0.0 < $1.0 : left < right
    }
    var nodes: [SettingsNode] = []
    for (key, fieldSchema) in ordered {
        guard matchesSection(fieldSchema, requested: requestedSection) else { continue }
        let fieldPath = path + [key]
        let fieldValue = current[key]
        if fieldSchema["type"] as? String == "object" {
            let children = buildNodes(
                schema: fieldSchema,
                current: fieldValue as? [String: Any] ?? [:],
                path: fieldPath,
                requestedSection: requestedSection
            )
            nodes.append(SettingsNode(.group(
                title: titleText(fieldSchema, fallback: key),
                description: descriptionText(fieldSchema),
                children: children
            )))
            continue
        }
        nodes.append(SettingsNode(.field(buildField(
            key: key,
            schema: fieldSchema,
            value: fieldValue,
            path: fieldPath
        ))))
    }
    return nodes
}

private func buildField(
    key: String,
    schema: [String: Any],
    value: Any?,
    path: [String]
) -> SettingsField {
    let kind: SettingsField.Kind
    if let choices = schema["enum"] as? [Any], !choices.isEmpty {
        let descriptions = schema["enumDescriptions"] as? [String]
        let labels = choices.enumerated().map { index, choice in
            descriptions?[safe: index] ?? String(describing: choice)
        }
        kind = .enumeration(choices: choices, labels: labels)
    } else {
        switch schema["type"] as? String {
        case "boolean": kind = .boolean
        case "integer": kind = .integer
        case "number": kind = .number
        case "array": kind = .array
        default: kind = .string(secret: schema["x-napplet-secret"] as? Bool == true)
        }
    }
    return SettingsField(
        path: path,
        key: path.joined(separator: "."),
        label: titleText(schema, fallback: key),
        description: descriptionText(schema),
        kind: kind,
        initiallyPresent: value != nil
    )
}

private func matchesSection(_ schema: [String: Any], requested: String?) -> Bool {
    guard let requested else { return true }
    if schema["x-napplet-section"] as? String == requested {
        return true
    }
    guard schema["type"] as? String == "object",
          let properties = schema["properties"] as? [String: Any]
    else {
        return false
    }
    return properties.values.contains {
        ($0 as? [String: Any]).isSomeAnd { matchesSection($0, requested: requested) }
    }
}

@MainActor
private final class SettingsFormStore: ObservableObject {
    @Published var stringValues: [String: String] = [:]
    @Published var boolValues: [String: Bool] = [:]
    @Published var enumIndices: [String: Int] = [:]
    @Published var changedKeys: Set<String> = []
    @Published var errorMessage: String?
    @Published var isSaving = false

    init(nodes: [SettingsNode], values: [String: Any]) {
        seed(nodes: nodes, values: values)
    }

    private func seed(nodes: [SettingsNode], values: [String: Any]) {
        for node in nodes {
            switch node.body {
            case let .field(field):
                let value = valueAt(field.path, in: values)
                switch field.kind {
                case .boolean:
                    boolValues[field.key] = (value as? Bool) == true
                case let .enumeration(choices, _):
                    if let value, let index = choices.firstIndex(where: { jsonEqual($0, value) }) {
                        enumIndices[field.key] = index
                    }
                case .array:
                    stringValues[field.key] = jsonString(value) ?? "[]"
                default:
                    stringValues[field.key] = value.map { String(describing: $0) } ?? ""
                }
            case let .group(_, _, children):
                seed(nodes: children, values: values)
            }
        }
    }

    private func valueAt(_ path: [String], in values: [String: Any]) -> Any? {
        guard let head = path.first else { return nil }
        if path.count == 1 { return values[head] }
        guard let nested = values[head] as? [String: Any] else { return nil }
        return valueAt(Array(path.dropFirst()), in: nested)
    }

    func read(_ field: SettingsField) throws -> Any {
        switch field.kind {
        case .string:
            return stringValues[field.key] ?? ""
        case .integer:
            guard let value = Int64(stringValues[field.key] ?? "") else {
                throw SettingsPresentationError.invalidInteger
            }
            return value
        case .number:
            guard let value = Double(stringValues[field.key] ?? ""), value.isFinite else {
                throw SettingsPresentationError.invalidNumber
            }
            return value
        case .boolean:
            return boolValues[field.key] ?? false
        case .array:
            let text = stringValues[field.key] ?? "[]"
            guard let data = text.data(using: .utf8),
                  let array = try? JSONSerialization.jsonObject(with: data) as? [Any]
            else {
                throw SettingsPresentationError.invalidArray
            }
            return array
        case let .enumeration(choices, _):
            guard let index = enumIndices[field.key], choices.indices.contains(index) else {
                throw SettingsPresentationError.invalidChoice
            }
            return choices[index]
        }
    }
}

private struct SettingsFieldRow: View {
    let field: SettingsField
    @ObservedObject var store: SettingsFormStore

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            control
            if let description = field.description {
                Text(description)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(field.label)
    }

    @ViewBuilder
    private var control: some View {
        switch field.kind {
        case let .string(secret):
            if secret {
                SecureField(field.label, text: binding(for: field.key))
            } else {
                TextField(field.label, text: binding(for: field.key))
            }
        case .integer, .number:
            TextField(field.label, text: binding(for: field.key))
                .keyboardType(.numbersAndPunctuation)
        case .array:
            TextField(field.label, text: binding(for: field.key))
                .autocapitalization(.none)
        case .boolean:
            Toggle(field.label, isOn: boolBinding(for: field.key))
        case let .enumeration(_, labels):
            Picker(field.label, selection: enumBinding(for: field.key)) {
                ForEach(Array(labels.enumerated()), id: \.offset) { index, label in
                    Text(label).tag(index)
                }
            }
        }
    }

    private func binding(for key: String) -> Binding<String> {
        Binding(
            get: { store.stringValues[key] ?? "" },
            set: {
                store.stringValues[key] = $0
                store.changedKeys.insert(key)
            }
        )
    }

    private func boolBinding(for key: String) -> Binding<Bool> {
        Binding(
            get: { store.boolValues[key] ?? false },
            set: {
                store.boolValues[key] = $0
                store.changedKeys.insert(key)
            }
        )
    }

    private func enumBinding(for key: String) -> Binding<Int> {
        Binding(
            get: { store.enumIndices[key] ?? 0 },
            set: {
                store.enumIndices[key] = $0
                store.changedKeys.insert(key)
            }
        )
    }
}

private struct SettingsNodeView: View {
    let node: SettingsNode
    @ObservedObject var store: SettingsFormStore

    var body: some View {
        switch node.body {
        case let .field(field):
            SettingsFieldRow(field: field, store: store)
        case let .group(title, description, children):
            GroupBox(title) {
                VStack(alignment: .leading, spacing: 12) {
                    if let description {
                        Text(description)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    ForEach(children) { child in
                        SettingsNodeView(node: child, store: store)
                    }
                }
            }
        }
    }
}

private struct SettingsFormView: View {
    let document: NativeSettingsDocument
    let nodes: [SettingsNode]
    let fields: [SettingsField]
    @ObservedObject var store: SettingsFormStore
    let onCommit: @Sendable ([String: Any], @escaping @Sendable (String?) -> Void) -> Void
    let onDismiss: () -> Void

    var body: some View {
        NavigationStack {
            Form {
                if let description = descriptionText(document.schema) {
                    Text(description)
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                ForEach(nodes) { node in
                    SettingsNodeView(node: node, store: store)
                }
                if let errorMessage = store.errorMessage {
                    Text(errorMessage)
                        .font(.footnote)
                        .foregroundStyle(.red)
                }
            }
            .navigationTitle(titleText(document.schema, fallback: "\(document.request.dTag) Settings"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", action: onDismiss)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save", action: save)
                        .disabled(store.isSaving)
                }
            }
        }
    }

    private func save() {
        var next = document.values
        do {
            for field in fields where field.initiallyPresent || store.changedKeys.contains(field.key) {
                let value = try store.read(field)
                set(value, at: field.path, in: &next)
            }
        } catch {
            store.errorMessage = error.localizedDescription
            return
        }
        store.isSaving = true
        store.errorMessage = nil
        onCommit(next) { error in
            DispatchQueue.main.async {
                store.isSaving = false
                if let error {
                    store.errorMessage = error
                } else {
                    onDismiss()
                }
            }
        }
    }

    private func set(_ value: Any, at path: [String], in object: inout [String: Any]) {
        guard let head = path.first else { return }
        if path.count == 1 {
            object[head] = value
            return
        }
        var nested = object[head] as? [String: Any] ?? [:]
        set(value, at: Array(path.dropFirst()), in: &nested)
        object[head] = nested
    }
}

private func flatten(_ nodes: [SettingsNode]) -> [SettingsField] {
    nodes.flatMap { node -> [SettingsField] in
        switch node.body {
        case let .field(field): [field]
        case let .group(_, _, children): flatten(children)
        }
    }
}

/// Finite UIKit settings executor. Rust supplies validated schema and current
/// values; this object presents a SwiftUI form and returns raw edits to Rust.
final class IOSSettingsExecutor: NativeSettingsExecutor, @unchecked Sendable {
    private static let maximumPresentations = 8

    private let lock = NSLock()
    private weak var controller: RuntimeController?
    private var pendingPresentations = 0
    private var isClosed = false
    private var presentations: [String: (sessionID: UInt64, controller: UIViewController)] = [:]

    func bind(controller: RuntimeController) {
        lock.lock()
        if !isClosed {
            self.controller = controller
        }
        lock.unlock()
    }

    func tryOpen(request: NativeSettingsRequest) -> NativeSettingsOpenResult {
        guard let document = NativeSettingsDocument.decode(request) else {
            return .unavailable
        }
        lock.lock()
        guard !isClosed else {
            lock.unlock()
            return .closed
        }
        guard presentations.count + pendingPresentations < Self.maximumPresentations else {
            lock.unlock()
            return .saturated
        }
        pendingPresentations += 1
        lock.unlock()
        DispatchQueue.main.async { [weak self] in
            self?.present(document)
        }
        return .accepted
    }

    func retainRunningSessions(_ sessionIDs: Set<UInt64>) {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            lock.lock()
            let stale = presentations.filter { !sessionIDs.contains($0.value.sessionID) }
            lock.unlock()
            for (key, entry) in stale {
                dismiss(key: key, controller: entry.controller)
            }
        }
    }

    func close() {
        lock.lock()
        guard !isClosed else {
            lock.unlock()
            return
        }
        isClosed = true
        controller = nil
        let active = presentations
        presentations.removeAll()
        lock.unlock()
        DispatchQueue.main.async {
            for (key, entry) in active {
                self.dismiss(key: key, controller: entry.controller)
            }
        }
    }

    @MainActor
    private func present(_ document: NativeSettingsDocument) {
        let key = Self.key(document.request)
        lock.lock()
        pendingPresentations = max(0, pendingPresentations - 1)
        guard !isClosed else {
            lock.unlock()
            return
        }
        guard presentations[key] == nil else {
            lock.unlock()
            return
        }
        guard let presenter = keyWindow()?.rootViewController else {
            lock.unlock()
            return
        }
        let nodes = buildNodes(
            schema: document.schema,
            current: document.values,
            path: [],
            requestedSection: document.request.section
        )
        let fields = flatten(nodes)
        let store = SettingsFormStore(nodes: nodes, values: document.values)
        let hosting = UIHostingController(
            rootView: SettingsFormView(
                document: document,
                nodes: nodes,
                fields: fields,
                store: store,
                onCommit: { [weak self] values, completion in
                    self?.commit(document.request, values: values, completion: completion)
                },
                onDismiss: { [weak self] in
                    self?.removePresentation(key)
                }
            )
        )
        hosting.modalPresentationStyle = .formSheet
        hosting.isModalInPresentation = true
        presentations[key] = (document.request.sessionId, hosting)
        lock.unlock()
        presenter.present(hosting, animated: true)
    }

    private func removePresentation(_ key: String) {
        lock.lock()
        let entry = presentations.removeValue(forKey: key)
        lock.unlock()
        if let entry {
            DispatchQueue.main.async {
                entry.controller.dismiss(animated: true)
            }
        }
    }

    @MainActor
    private func dismiss(key: String, controller: UIViewController) {
        lock.lock()
        presentations.removeValue(forKey: key)
        lock.unlock()
        controller.dismiss(animated: true)
    }

    private func commit(
        _ request: NativeSettingsRequest,
        values: [String: Any],
        completion: @escaping @Sendable (String?) -> Void
    ) {
        guard JSONSerialization.isValidJSONObject(values),
              let data = try? JSONSerialization.data(
                  withJSONObject: values,
                  options: [.sortedKeys]
              ),
              data.count <= 192 * 1_024,
              let valuesJSON = String(data: data, encoding: .utf8)
        else {
            completion("The edited settings could not be encoded.")
            return
        }
        lock.lock()
        let controller = isClosed ? nil : controller
        lock.unlock()
        guard let controller else {
            completion("The runtime settings capability is closed.")
            return
        }
        DispatchQueue.global(qos: .userInitiated).async {
            let update = controller.commitConfigValues(
                commit: NativeConfigCommit(
                    manifestAuthor: request.manifestAuthor,
                    dTag: request.dTag,
                    aggregateHash: request.aggregateHash,
                    sessionId: request.sessionId,
                    valuesJson: valuesJSON
                )
            )
            completion(update.accepted ? nil : update.refusal?.detail ?? "Settings were refused.")
        }
    }

    private static func key(_ request: NativeSettingsRequest) -> String {
        [
            request.manifestAuthor,
            request.dTag,
            request.aggregateHash,
            String(request.sessionId),
        ].joined(separator: ":")
    }
}
#endif
