import AppKit
import Foundation
import NMPNativeRuntime

/// Event-driven projection of AppKit appearance facts. The callback reports
/// raw OS traits only; Rust maps them to the pinned NAP-THEME schema.
final class MacOSAppearanceSource: NSObject, NativeAppearanceSource, @unchecked Sendable {
    private let lock = NSLock()
    private var snapshot: NativeAppearanceSnapshot
    private weak var controller: RuntimeController?
    private var isClosed = false
    private var refreshPending = false
    private var appearanceObservation: NSKeyValueObservation?
    private var accessibilityObserver: NSObjectProtocol?
    private var colorObserver: NSObjectProtocol?

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
        guard appearanceObservation == nil else { return }
        appearanceObservation = NSApplication.shared.observe(
            \.effectiveAppearance,
            options: [.new]
        ) { [weak self] _, _ in
            self?.scheduleRefresh()
        }
        accessibilityObserver = NotificationCenter.default.addObserver(
            forName: NSWorkspace.accessibilityDisplayOptionsDidChangeNotification,
            object: NSWorkspace.shared,
            queue: .main
        ) { [weak self] _ in
            self?.scheduleRefresh()
        }
        colorObserver = NotificationCenter.default.addObserver(
            forName: NSColor.systemColorsDidChangeNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.scheduleRefresh()
        }
    }

    @MainActor
    private func removeObservers() {
        appearanceObservation?.invalidate()
        appearanceObservation = nil
        if let accessibilityObserver {
            NotificationCenter.default.removeObserver(accessibilityObserver)
            self.accessibilityObserver = nil
        }
        if let colorObserver {
            NotificationCenter.default.removeObserver(colorObserver)
            self.colorObserver = nil
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
        let appearance = NSApplication.shared.effectiveAppearance
        let dark = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
        let workspace = NSWorkspace.shared
        let accent = NSColor.controlAccentColor.usingColorSpace(.sRGB) ?? .systemBlue
        return NativeAppearanceSnapshot(
            dark: dark,
            increasedContrast: workspace.accessibilityDisplayShouldIncreaseContrast,
            reducedTransparency: workspace.accessibilityDisplayShouldReduceTransparency,
            accentRed: component(accent.redComponent),
            accentGreen: component(accent.greenComponent),
            accentBlue: component(accent.blueComponent)
        )
    }

    private static func component(_ value: CGFloat) -> UInt8 {
        UInt8(clamping: Int((min(max(value, 0), 1) * 255).rounded()))
    }
}

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

/// Finite AppKit settings executor. Rust supplies validated schema and current
/// values; this object renders controls and returns raw edits to Rust.
final class MacOSSettingsExecutor: NativeSettingsExecutor, @unchecked Sendable {
    private static let maximumWindows = 8

    private let lock = NSLock()
    private weak var controller: RuntimeController?
    private var pendingPresentations = 0
    private var isClosed = false
    private var windows: [String: NativeSettingsWindowController] = [:]

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
        guard windows.count + pendingPresentations < Self.maximumWindows else {
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
            let stale = windows.filter {
                !sessionIDs.contains($0.value.sessionID)
            }.map(\.value)
            lock.unlock()
            for window in stale {
                window.close()
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
        let active = Array(windows.values)
        windows.removeAll()
        lock.unlock()
        DispatchQueue.main.async {
            for window in active {
                window.close()
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
        if let existing = windows[key] {
            lock.unlock()
            existing.showWindow(nil)
            existing.window?.makeKeyAndOrderFront(nil)
            return
        }
        let window = NativeSettingsWindowController(
            document: document,
            onCommit: { [weak self] values, completion in
                self?.commit(document.request, values: values, completion: completion)
            },
            onClose: { [weak self] in
                self?.removeWindow(key)
            }
        )
        windows[key] = window
        lock.unlock()
        window.showWindow(nil)
        window.window?.makeKeyAndOrderFront(nil)
        NSApplication.shared.activate(ignoringOtherApps: true)
    }

    private func removeWindow(_ key: String) {
        lock.lock()
        windows.removeValue(forKey: key)
        lock.unlock()
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

private final class NativeSettingsWindowController: NSWindowController, NSWindowDelegate {
    let sessionID: UInt64
    private let onClose: @Sendable () -> Void

    init(
        document: NativeSettingsDocument,
        onCommit: @escaping @Sendable ([String: Any], @escaping @Sendable (String?) -> Void) -> Void,
        onClose: @escaping @Sendable () -> Void
    ) {
        sessionID = document.request.sessionId
        self.onClose = onClose
        let content = NativeSettingsViewController(document: document, onCommit: onCommit)
        let window = NSWindow(contentViewController: content)
        window.title = "\(document.request.dTag) Settings"
        window.styleMask = [.titled, .closable, .miniaturizable, .resizable]
        window.setContentSize(NSSize(width: 560, height: 520))
        window.minSize = NSSize(width: 440, height: 360)
        window.isReleasedWhenClosed = false
        super.init(window: window)
        window.delegate = self
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        nil
    }

    func windowWillClose(_ notification: Notification) {
        onClose()
    }
}

private final class NativeSettingsViewController: NSViewController {
    private enum FieldKind {
        case string
        case integer
        case number
        case boolean
        case array
        case enumeration([Any])
    }

    private final class FieldBinding {
        let path: [String]
        let kind: FieldKind
        let control: NSControl
        let initiallyPresent: Bool
        var changed = false

        init(path: [String], kind: FieldKind, control: NSControl, initiallyPresent: Bool) {
            self.path = path
            self.kind = kind
            self.control = control
            self.initiallyPresent = initiallyPresent
        }
    }

    private let document: NativeSettingsDocument
    private let onCommit:
        @Sendable ([String: Any], @escaping @Sendable (String?) -> Void) -> Void
    private var bindings: [FieldBinding] = []
    private var values: [String: Any]
    private let errorLabel = NSTextField(labelWithString: "")
    private let saveButton = NSButton(title: "Save", target: nil, action: nil)

    init(
        document: NativeSettingsDocument,
        onCommit: @escaping @Sendable ([String: Any], @escaping @Sendable (String?) -> Void) -> Void
    ) {
        self.document = document
        self.onCommit = onCommit
        values = document.values
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        nil
    }

    override func loadView() {
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 14
        stack.edgeInsets = NSEdgeInsets(top: 20, left: 20, bottom: 20, right: 20)

        let title = NSTextField(labelWithString: titleText(document.schema, fallback: "Settings"))
        title.font = .preferredFont(forTextStyle: .title2)
        title.setAccessibilityRole(.staticText)
        stack.addArrangedSubview(title)
        if let description = descriptionText(document.schema) {
            let label = wrappingLabel(description)
            stack.addArrangedSubview(label)
        }
        addObject(
            schema: document.schema,
            current: document.values,
            path: [],
            to: stack,
            requestedSection: document.request.section
        )

        errorLabel.textColor = .systemRed
        errorLabel.maximumNumberOfLines = 3
        errorLabel.lineBreakMode = .byWordWrapping
        errorLabel.isHidden = true
        stack.addArrangedSubview(errorLabel)

        let buttons = NSStackView()
        buttons.orientation = .horizontal
        buttons.spacing = 8
        let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancel))
        cancel.keyEquivalent = "\u{1b}"
        saveButton.target = self
        saveButton.action = #selector(save)
        saveButton.keyEquivalent = "\r"
        buttons.addArrangedSubview(cancel)
        buttons.addArrangedSubview(saveButton)
        stack.addArrangedSubview(buttons)

        let documentView = NSView()
        documentView.translatesAutoresizingMaskIntoConstraints = false
        stack.translatesAutoresizingMaskIntoConstraints = false
        documentView.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: documentView.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: documentView.trailingAnchor),
            stack.topAnchor.constraint(equalTo: documentView.topAnchor),
            stack.bottomAnchor.constraint(equalTo: documentView.bottomAnchor),
            stack.widthAnchor.constraint(greaterThanOrEqualToConstant: 400),
        ])
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.documentView = documentView
        view = scroll
    }

    private func addObject(
        schema: [String: Any],
        current: [String: Any],
        path: [String],
        to stack: NSStackView,
        requestedSection: String?
    ) {
        let properties = schema["properties"] as? [String: Any] ?? [:]
        let ordered = properties.compactMap { key, value -> (String, [String: Any])? in
            guard let value = value as? [String: Any] else { return nil }
            return (key, value)
        }.sorted {
            let left = ($0.1["x-napplet-order"] as? NSNumber)?.doubleValue ?? .greatestFiniteMagnitude
            let right = ($1.1["x-napplet-order"] as? NSNumber)?.doubleValue ?? .greatestFiniteMagnitude
            return left == right ? $0.0 < $1.0 : left < right
        }
        for (key, fieldSchema) in ordered {
            guard matchesSection(fieldSchema, requested: requestedSection) else { continue }
            let fieldPath = path + [key]
            let fieldValue = current[key]
            if fieldSchema["type"] as? String == "object" {
                let nested = NSStackView()
                nested.orientation = .vertical
                nested.alignment = .leading
                nested.spacing = 10
                let heading = NSTextField(
                    labelWithString: titleText(fieldSchema, fallback: key)
                )
                heading.font = .preferredFont(forTextStyle: .headline)
                heading.setAccessibilityRole(.staticText)
                nested.addArrangedSubview(heading)
                if let description = descriptionText(fieldSchema) {
                    nested.addArrangedSubview(wrappingLabel(description))
                }
                addObject(
                    schema: fieldSchema,
                    current: fieldValue as? [String: Any] ?? [:],
                    path: fieldPath,
                    to: nested,
                    requestedSection: requestedSection
                )
                let box = NSBox()
                box.boxType = .primary
                box.contentView = nested
                stack.addArrangedSubview(box)
                box.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
                continue
            }
            addField(
                key: key,
                schema: fieldSchema,
                value: fieldValue,
                path: fieldPath,
                to: stack
            )
        }
    }

    private func addField(
        key: String,
        schema: [String: Any],
        value: Any?,
        path: [String],
        to stack: NSStackView
    ) {
        let row = NSStackView()
        row.orientation = .horizontal
        row.alignment = .firstBaseline
        row.spacing = 12
        let label = NSTextField(labelWithString: titleText(schema, fallback: key))
        label.alignment = .right
        label.widthAnchor.constraint(equalToConstant: 150).isActive = true
        row.addArrangedSubview(label)

        let control: NSControl
        let kind: FieldKind
        if let choices = schema["enum"] as? [Any], !choices.isEmpty {
            let popup = NSPopUpButton()
            let descriptions = schema["enumDescriptions"] as? [String]
            popup.addItems(withTitles: choices.enumerated().map { index, choice in
                descriptions?[safe: index] ?? String(describing: choice)
            })
            if let value,
               let selected = choices.firstIndex(where: { jsonEqual($0, value) }) {
                popup.selectItem(at: selected)
            }
            control = popup
            kind = .enumeration(choices)
        } else {
            switch schema["type"] as? String {
            case "boolean":
                let toggle = NSSwitch()
                toggle.state = (value as? Bool) == true ? .on : .off
                control = toggle
                kind = .boolean
            case "integer":
                let input = NSTextField(string: value.map(String.init(describing:)) ?? "")
                input.alignment = .right
                control = input
                kind = .integer
            case "number":
                let input = NSTextField(string: value.map(String.init(describing:)) ?? "")
                input.alignment = .right
                control = input
                kind = .number
            case "array":
                let input = NSTextField(string: jsonString(value) ?? "[]")
                input.placeholderString = "JSON array"
                control = input
                kind = .array
            default:
                let secret = schema["x-napplet-secret"] as? Bool == true
                let input: NSTextField = secret
                    ? NSSecureTextField(string: value as? String ?? "")
                    : NSTextField(string: value as? String ?? "")
                control = input
                kind = .string
            }
        }
        control.target = self
        control.action = #selector(fieldChanged(_:))
        control.setAccessibilityLabel(label.stringValue)
        if let description = descriptionText(schema) {
            control.setAccessibilityHelp(description)
        }
        control.widthAnchor.constraint(greaterThanOrEqualToConstant: 220).isActive = true
        row.addArrangedSubview(control)
        stack.addArrangedSubview(row)
        row.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        if let description = descriptionText(schema) {
            let help = wrappingLabel(description)
            help.textColor = .secondaryLabelColor
            help.font = .preferredFont(forTextStyle: .caption1)
            stack.addArrangedSubview(help)
        }
        bindings.append(
            FieldBinding(
                path: path,
                kind: kind,
                control: control,
                initiallyPresent: value != nil
            )
        )
    }

    @objc private func fieldChanged(_ sender: NSControl) {
        bindings.first(where: { $0.control === sender })?.changed = true
    }

    @objc private func cancel() {
        view.window?.close()
    }

    @objc private func save() {
        do {
            var next = values
            for binding in bindings where binding.initiallyPresent || binding.changed {
                let value = try read(binding)
                set(value, at: binding.path, in: &next)
            }
            saveButton.isEnabled = false
            errorLabel.isHidden = true
            onCommit(next) { [weak self] error in
                DispatchQueue.main.async {
                    guard let self else { return }
                    if let error {
                        self.errorLabel.stringValue = error
                        self.errorLabel.isHidden = false
                        self.saveButton.isEnabled = true
                    } else {
                        self.values.removeAll(keepingCapacity: false)
                        self.view.window?.close()
                    }
                }
            }
        } catch {
            errorLabel.stringValue = error.localizedDescription
            errorLabel.isHidden = false
        }
    }

    private func read(_ binding: FieldBinding) throws -> Any {
        switch binding.kind {
        case .string:
            return (binding.control as? NSTextField)?.stringValue ?? ""
        case .integer:
            guard let value = Int64((binding.control as? NSTextField)?.stringValue ?? "") else {
                throw SettingsPresentationError.invalidInteger
            }
            return value
        case .number:
            guard let value = Double((binding.control as? NSTextField)?.stringValue ?? ""),
                  value.isFinite
            else {
                throw SettingsPresentationError.invalidNumber
            }
            return value
        case .boolean:
            return (binding.control as? NSSwitch)?.state == .on
        case .array:
            let text = (binding.control as? NSTextField)?.stringValue ?? "[]"
            guard let data = text.data(using: .utf8),
                  let array = try? JSONSerialization.jsonObject(with: data) as? [Any]
            else {
                throw SettingsPresentationError.invalidArray
            }
            return array
        case let .enumeration(choices):
            let index = (binding.control as? NSPopUpButton)?.indexOfSelectedItem ?? -1
            guard choices.indices.contains(index) else {
                throw SettingsPresentationError.invalidChoice
            }
            return choices[index]
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
}

private enum SettingsPresentationError: LocalizedError {
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

private func titleText(_ schema: [String: Any], fallback: String) -> String {
    (schema["title"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? fallback
}

private func descriptionText(_ schema: [String: Any]) -> String? {
    for key in ["markdownDescription", "description", "deprecationMessage"] {
        if let value = schema[key] as? String, !value.isEmpty {
            return value
        }
    }
    return nil
}

@MainActor
private func wrappingLabel(_ value: String) -> NSTextField {
    let label = NSTextField(wrappingLabelWithString: value)
    label.maximumNumberOfLines = 0
    return label
}

private func jsonEqual(_ left: Any, _ right: Any) -> Bool {
    guard JSONSerialization.isValidJSONObject([left]),
          JSONSerialization.isValidJSONObject([right]),
          let lhs = try? JSONSerialization.data(withJSONObject: [left], options: [.sortedKeys]),
          let rhs = try? JSONSerialization.data(withJSONObject: [right], options: [.sortedKeys])
    else {
        return false
    }
    return lhs == rhs
}

private func jsonString(_ value: Any?) -> String? {
    guard let value,
          JSONSerialization.isValidJSONObject(value),
          let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    else {
        return nil
    }
    return String(data: data, encoding: .utf8)
}

private extension Array {
    subscript(safe index: Index) -> Element? {
        indices.contains(index) ? self[index] : nil
    }
}

private extension Optional {
    func isSomeAnd(_ predicate: (Wrapped) -> Bool) -> Bool {
        map(predicate) ?? false
    }
}
