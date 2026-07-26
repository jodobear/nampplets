import Darwin
import Foundation

struct AppleProcessMeasurement: Equatable, Sendable {
    let cpuTimeNanoseconds: UInt64?
    let peakRSSBytes: UInt64?

    static func capture() -> Self {
        var usage = rusage()
        guard getrusage(RUSAGE_SELF, &usage) == 0 else {
            return Self(cpuTimeNanoseconds: nil, peakRSSBytes: nil)
        }
        let user = nanoseconds(usage.ru_utime)
        let system = nanoseconds(usage.ru_stime)
        return Self(
            cpuTimeNanoseconds: user.flatMap { user in
                system.flatMap { system in
                    user.addingReportingOverflow(system).overflow
                        ? nil
                        : user + system
                }
            },
            peakRSSBytes: UInt64(max(0, usage.ru_maxrss))
        )
    }

    private static func nanoseconds(_ value: timeval) -> UInt64? {
        guard value.tv_sec >= 0, value.tv_usec >= 0 else {
            return nil
        }
        let seconds = UInt64(value.tv_sec)
        let microseconds = UInt64(value.tv_usec)
        let (secondNanoseconds, secondsOverflow) =
            seconds.multipliedReportingOverflow(by: 1_000_000_000)
        let (microsecondNanoseconds, microsOverflow) =
            microseconds.multipliedReportingOverflow(by: 1_000)
        let (total, totalOverflow) =
            secondNanoseconds.addingReportingOverflow(
                microsecondNanoseconds
            )
        return secondsOverflow || microsOverflow || totalOverflow
            ? nil
            : total
    }
}

struct ApplePerformanceEnvironment: Equatable, Sendable {
    let environmentClass: String
    let operatingSystem: String
    let hardware: String
    let powerState: String
    let thermalState: String
    let cpuTimeAvailable: Bool
    let peakRSSAvailable: Bool

    static func capture() -> Self {
        let measurement = AppleProcessMeasurement.capture()
        return Self(
            environmentClass: "ordinary-apple-test-host",
            operatingSystem:
                ProcessInfo.processInfo.operatingSystemVersionString,
            hardware: machineIdentifier(),
            powerState: "unknown",
            thermalState: thermalState(),
            cpuTimeAvailable: measurement.cpuTimeNanoseconds != nil,
            peakRSSAvailable: measurement.peakRSSBytes != nil
        )
    }

    private static func machineIdentifier() -> String {
        var value = utsname()
        uname(&value)
        return withUnsafePointer(to: &value.machine) { pointer in
            pointer.withMemoryRebound(to: CChar.self, capacity: 1) {
                String(cString: $0)
            }
        }
    }

    private static func thermalState() -> String {
        switch ProcessInfo.processInfo.thermalState {
        case .nominal: "nominal"
        case .fair: "fair"
        case .serious: "serious"
        case .critical: "critical"
        @unknown default: "unknown"
        }
    }
}
