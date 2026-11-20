import CoreMotion
import Foundation
import QuartzCore

nonisolated struct InertialEvidence: Encodable, Sendable {
    struct Series: Encodable, Sendable {
        let availability: String
        let units: String
        let sampleCount: Int
        let observedSpanSeconds: Double
        let observedSamplesPerSecond: Double?
        let longestGapSeconds: Double?
        let samples: [[Double]]
    }

    let schemaVersion = 1
    let requestedSamplesPerSecond = 100
    let sampleColumns = ["offsetSecondsFromCapture", "x", "y", "z"]
    let timeBasis = "Core Motion sensor uptime minus the capture window's host-uptime center"
    let axes = "Core Motion device axes; no rotation into display orientation or ARKit world space"
    let accelerometer: Series
    let gyroscope: Series
}

@MainActor
final class InertialEvidenceCollector {
    private struct Sample {
        let time: Double
        let xyz: SIMD3<Double>
    }

    private struct Buffer {
        var samples: [Sample] = []
        var clockMismatch = false
        var hadError = false

        mutating func append(time: Double, x: Double, y: Double, z: Double, retention: Double) {
            guard time.isFinite, x.isFinite, y.isFinite, z.isFinite else { return }
            guard abs(CACurrentMediaTime() - time) < 1 else {
                clockMismatch = true
                return
            }
            guard samples.last.map({ time > $0.time }) ?? true else { return }
            samples.append(Sample(time: time, xyz: SIMD3(x, y, z)))
            if samples.count > 3_000 { samples.removeFirst(500) }
            if let first = samples.first, first.time < time - retention - 1 {
                samples.removeAll { $0.time < time - retention }
            }
        }

        func export(available: Bool, units: String, center: Double, start: Double, end: Double) -> InertialEvidence.Series {
            let selected = samples.filter { (start...end).contains($0.time) }
            let span = (selected.last?.time ?? 0) - (selected.first?.time ?? 0)
            let maxGap = zip(selected, selected.dropFirst()).map { $1.time - $0.time }.max()
            let availability: String
            if !available { availability = "unavailable" }
            else if selected.isEmpty { availability = clockMismatch ? "clockMismatch" : hadError ? "sensorError" : "noSamples" }
            else { availability = span >= end - start - 0.2 && (maxGap ?? .infinity) < 0.2 ? "fullWindow" : "partialWindow" }
            return InertialEvidence.Series(availability: availability, units: units, sampleCount: selected.count,
                                           observedSpanSeconds: span,
                                           observedSamplesPerSecond: span > 0 ? Double(selected.count - 1) / span : nil,
                                           longestGapSeconds: maxGap,
                                           samples: selected.map { [$0.time - center, $0.xyz.x, $0.xyz.y, $0.xyz.z] })
        }
    }

    private let manager = CMMotionManager()
    private var accelerometer = Buffer()
    private var gyroscope = Buffer()
    private var retention = 12.0
    private var generation = 0

    func configure(retention: Double) { self.retention = retention }

    func setActive(_ active: Bool, preserveWindow: Bool) {
        generation += 1
        let current = generation
        guard active else {
            manager.stopAccelerometerUpdates()
            manager.stopGyroUpdates()
            if !preserveWindow { clear() }
            return
        }
        manager.accelerometerUpdateInterval = 0.01
        manager.gyroUpdateInterval = 0.01
        if manager.isAccelerometerAvailable {
            manager.startAccelerometerUpdates(to: .main) { [weak self] data, error in
                MainActor.assumeIsolated {
                    guard let self, self.generation == current else { return }
                    if error != nil { self.accelerometer.hadError = true }
                    guard let data else { return }
                    let a = data.acceleration
                    self.accelerometer.append(time: data.timestamp, x: a.x, y: a.y, z: a.z, retention: self.retention)
                }
            }
        }
        if manager.isGyroAvailable {
            manager.startGyroUpdates(to: .main) { [weak self] data, error in
                MainActor.assumeIsolated {
                    guard let self, self.generation == current else { return }
                    if error != nil { self.gyroscope.hadError = true }
                    guard let data else { return }
                    let r = data.rotationRate
                    self.gyroscope.append(time: data.timestamp, x: r.x, y: r.y, z: r.z, retention: self.retention)
                }
            }
        }
    }

    func finish(center: Double, start: Double, end: Double) -> InertialEvidence {
        defer { clear() }
        return InertialEvidence(accelerometer: accelerometer.export(available: manager.isAccelerometerAvailable,
                                                                      units: "g, includes gravity", center: center, start: start, end: end),
                                gyroscope: gyroscope.export(available: manager.isGyroAvailable, units: "radians/second",
                                                             center: center, start: start, end: end))
    }

    private func clear() {
        accelerometer = Buffer()
        gyroscope = Buffer()
    }
}
