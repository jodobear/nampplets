public enum CatalogRequestGenerationLane:
    String,
    Equatable,
    Sendable
{
    case feed
    case transientOperation
}

public struct CatalogRequestGenerationExhaustion:
    Equatable,
    Sendable
{
    public let lane: CatalogRequestGenerationLane
    public let exhaustedGeneration: UInt

    public init(
        lane: CatalogRequestGenerationLane,
        exhaustedGeneration: UInt
    ) {
        self.lane = lane
        self.exhaustedGeneration = exhaustedGeneration
    }

    public var technicalDetail: String {
        "\(lane.rawValue) request generation exhausted at "
            + "\(exhaustedGeneration)"
    }
}

struct CatalogRequestGenerationCounter: Sendable {
    let lane: CatalogRequestGenerationLane
    private(set) var current: UInt
    private(set) var exhaustion: CatalogRequestGenerationExhaustion?

    init(
        lane: CatalogRequestGenerationLane,
        current: UInt = 0
    ) {
        self.lane = lane
        self.current = current
    }

    mutating func issue() -> UInt? {
        guard exhaustion == nil else {
            return nil
        }
        let (next, overflow) = current.addingReportingOverflow(1)
        guard !overflow else {
            exhaustion = CatalogRequestGenerationExhaustion(
                lane: lane,
                exhaustedGeneration: current
            )
            return nil
        }
        current = next
        return next
    }

    func isCurrent(_ generation: UInt) -> Bool {
        exhaustion == nil && current == generation
    }
}
