import CoreGraphics
import Foundation

public enum WorkbenchSlotRole: String, CaseIterable, Codable, Hashable, Sendable {
    case feed
    case detail
    case composer
    case tool

    public var title: String {
        switch self {
        case .feed: "Feed"
        case .detail: "Detail"
        case .composer: "Composer"
        case .tool: "Tool"
        }
    }

    public var systemImage: String {
        switch self {
        case .feed: "rectangle.stack"
        case .detail: "sidebar.right"
        case .composer: "square.and.pencil"
        case .tool: "wrench.and.screwdriver"
        }
    }

    public var constraints: WorkbenchSlotConstraints {
        switch self {
        case .feed:
            WorkbenchSlotConstraints(
                minimumWidth: 320,
                idealWidth: 480,
                maximumWidth: 960,
                minimumHeight: 240,
                idealHeight: 480,
                maximumHeight: 1_200
            )
        case .detail:
            WorkbenchSlotConstraints(
                minimumWidth: 260,
                idealWidth: 320,
                maximumWidth: 720,
                minimumHeight: 240,
                idealHeight: 480,
                maximumHeight: 1_200
            )
        case .composer:
            WorkbenchSlotConstraints(
                minimumWidth: 480,
                idealWidth: 900,
                maximumWidth: 2_400,
                minimumHeight: 140,
                idealHeight: 190,
                maximumHeight: 420
            )
        case .tool:
            WorkbenchSlotConstraints(
                minimumWidth: 240,
                idealWidth: 280,
                maximumWidth: 560,
                minimumHeight: 240,
                idealHeight: 480,
                maximumHeight: 1_200
            )
        }
    }
}

public enum WorkbenchComponentID: String, Codable, Hashable, Sendable {
    case goodMorning = "good-morning"

    public var title: String {
        switch self {
        case .goodMorning: "Good Morning"
        }
    }
}

public struct WorkbenchSlotConstraints: Equatable, Sendable {
    public let minimumWidth: CGFloat
    public let idealWidth: CGFloat
    public let maximumWidth: CGFloat
    public let minimumHeight: CGFloat
    public let idealHeight: CGFloat
    public let maximumHeight: CGFloat

    public init(
        minimumWidth: CGFloat,
        idealWidth: CGFloat,
        maximumWidth: CGFloat,
        minimumHeight: CGFloat,
        idealHeight: CGFloat,
        maximumHeight: CGFloat
    ) {
        self.minimumWidth = minimumWidth
        self.idealWidth = idealWidth
        self.maximumWidth = maximumWidth
        self.minimumHeight = minimumHeight
        self.idealHeight = idealHeight
        self.maximumHeight = maximumHeight
    }

    public func clamped(width: Double, height: Double) -> WorkbenchSlotSize {
        WorkbenchSlotSize(
            width: min(max(width, minimumWidth), maximumWidth),
            height: min(max(height, minimumHeight), maximumHeight)
        )
    }
}

public struct WorkbenchSlotSize: Codable, Equatable, Sendable {
    public var width: Double
    public var height: Double

    public init(width: Double, height: Double) {
        self.width = width
        self.height = height
    }
}

public struct WorkbenchLayoutSnapshot: Codable, Equatable, Sendable {
    public static let currentVersion = 1

    public var version: Int
    public var visibleRoles: Set<WorkbenchSlotRole>
    public var assignments: [WorkbenchSlotRole: WorkbenchComponentID]
    public var focusedRole: WorkbenchSlotRole?
    public var sizes: [WorkbenchSlotRole: WorkbenchSlotSize]

    public init(
        version: Int = currentVersion,
        visibleRoles: Set<WorkbenchSlotRole>,
        assignments: [WorkbenchSlotRole: WorkbenchComponentID],
        focusedRole: WorkbenchSlotRole?,
        sizes: [WorkbenchSlotRole: WorkbenchSlotSize]
    ) {
        self.version = version
        self.visibleRoles = visibleRoles
        self.assignments = assignments
        self.focusedRole = focusedRole
        self.sizes = sizes
    }

    public static var workbenchDefault: WorkbenchLayoutSnapshot {
        WorkbenchLayoutSnapshot(
            visibleRoles: Set(WorkbenchSlotRole.allCases),
            assignments: [.feed: .goodMorning],
            focusedRole: .feed,
            sizes: Dictionary(
                uniqueKeysWithValues: WorkbenchSlotRole.allCases.map { role in
                    let constraints = role.constraints
                    return (
                        role,
                        WorkbenchSlotSize(
                            width: constraints.idealWidth,
                            height: constraints.idealHeight
                        )
                    )
                }
            )
        )
    }
}

public struct WorkbenchLayoutModel: Equatable, Sendable {
    public private(set) var snapshot: WorkbenchLayoutSnapshot

    public init(snapshot: WorkbenchLayoutSnapshot = .workbenchDefault) {
        self.snapshot = Self.normalized(snapshot)
    }

    public func isVisible(_ role: WorkbenchSlotRole) -> Bool {
        snapshot.visibleRoles.contains(role)
    }

    public func component(in role: WorkbenchSlotRole) -> WorkbenchComponentID? {
        snapshot.assignments[role]
    }

    public func size(for role: WorkbenchSlotRole) -> WorkbenchSlotSize {
        snapshot.sizes[role] ?? role.constraints.clamped(
            width: role.constraints.idealWidth,
            height: role.constraints.idealHeight
        )
    }

    public mutating func move(
        _ component: WorkbenchComponentID,
        to role: WorkbenchSlotRole
    ) {
        for existingRole in WorkbenchSlotRole.allCases
        where snapshot.assignments[existingRole] == component {
            snapshot.assignments.removeValue(forKey: existingRole)
        }
        snapshot.assignments[role] = component
        snapshot.visibleRoles.insert(role)
        snapshot.focusedRole = role
    }

    public mutating func setVisible(
        _ isVisible: Bool,
        role: WorkbenchSlotRole
    ) {
        if isVisible {
            snapshot.visibleRoles.insert(role)
        } else {
            snapshot.visibleRoles.remove(role)
            if snapshot.focusedRole == role {
                snapshot.focusedRole = WorkbenchSlotRole.allCases.first {
                    snapshot.visibleRoles.contains($0)
                }
            }
        }
    }

    public mutating func focus(_ role: WorkbenchSlotRole) {
        snapshot.visibleRoles.insert(role)
        snapshot.focusedRole = role
    }

    @discardableResult
    public mutating func recordRenderedSize(
        role: WorkbenchSlotRole,
        width: Double,
        height: Double
    ) -> Bool {
        let clamped = role.constraints.clamped(width: width, height: height)
        guard size(for: role) != clamped else {
            return false
        }
        snapshot.sizes[role] = clamped
        return true
    }

    private static func normalized(
        _ candidate: WorkbenchLayoutSnapshot
    ) -> WorkbenchLayoutSnapshot {
        guard candidate.version == WorkbenchLayoutSnapshot.currentVersion else {
            return .workbenchDefault
        }

        var result = candidate
        var assignedComponents = Set<WorkbenchComponentID>()
        for role in WorkbenchSlotRole.allCases {
            let proposed = result.sizes[role] ?? WorkbenchSlotSize(
                width: role.constraints.idealWidth,
                height: role.constraints.idealHeight
            )
            result.sizes[role] = role.constraints.clamped(
                width: proposed.width,
                height: proposed.height
            )

            guard let component = result.assignments[role] else {
                continue
            }
            if assignedComponents.contains(component) {
                result.assignments.removeValue(forKey: role)
            } else {
                assignedComponents.insert(component)
            }
        }

        if let focusedRole = result.focusedRole,
           !result.visibleRoles.contains(focusedRole) {
            result.focusedRole = WorkbenchSlotRole.allCases.first {
                result.visibleRoles.contains($0)
            }
        }
        return result
    }
}

/// The Rust workspace adapter implements this protocol. The feature deliberately
/// has no UserDefaults, AppStorage, or SceneStorage fallback.
@MainActor
public protocol WorkbenchLayoutPersisting {
    func loadLayout(workspaceID: String) throws -> WorkbenchLayoutSnapshot?
    func saveLayout(
        _ snapshot: WorkbenchLayoutSnapshot,
        workspaceID: String
    ) throws
}

@MainActor
public struct VolatileWorkbenchLayoutStore: WorkbenchLayoutPersisting {
    public init() {}

    public func loadLayout(workspaceID: String) throws -> WorkbenchLayoutSnapshot? {
        nil
    }

    public func saveLayout(
        _ snapshot: WorkbenchLayoutSnapshot,
        workspaceID: String
    ) throws {}
}
