import CoreMotion
import Foundation
import QuartzCore
import os

nonisolated struct MotionSample: Sendable {
    let timestamp: Double
    let arrivalLag: Double
    let rotation: SIMD3<Double>
    let gravity: SIMD3<Double>
    let acceleration: SIMD3<Double>

    init(_ motion: CMDeviceMotion) {
        timestamp = motion.timestamp
        arrivalLag = CACurrentMediaTime() - timestamp
        let r = motion.rotationRate, g = motion.gravity, a = motion.userAcceleration
        rotation = SIMD3(r.x, r.y, r.z)
        gravity = SIMD3(g.x, g.y, g.z)
        acceleration = SIMD3(a.x, a.y, a.z)
    }

    var encoded: [Double] {
        [timestamp, rotation.x, rotation.y, rotation.z, gravity.x, gravity.y, gravity.z,
         acceleration.x, acceleration.y, acceleration.z]
    }
}

@MainActor
final class MotionWindow {
    private nonisolated struct Buffer: Sendable {
        var slots = [MotionSample?](repeating: nil, count: 256)
        var count = 0
        var next = 0
        var failed = false
        var generation = UUID()

        mutating func append(_ sample: MotionSample) {
            if let last = slots[(next + slots.count - 1) % slots.count], sample.timestamp <= last.timestamp { return }
            slots[next] = sample
            next = (next + 1) % slots.count
            count = min(count + 1, slots.count)
        }

        func samples() -> [MotionSample] {
            (0..<count).compactMap { slots[(next + slots.count - count + $0) % slots.count] }
        }
    }

    private let manager = CMMotionManager()
    private let queue: OperationQueue = {
        let queue = OperationQueue()
        queue.name = "org.galaxy.capture.motion"
        queue.qualityOfService = .userInitiated
        queue.maxConcurrentOperationCount = 1
        return queue
    }()
    private let storage = OSAllocatedUnfairLock(initialState: Buffer())

    var status: String {
        guard manager.isDeviceMotionAvailable else { return "Unavailable" }
        if storage.withLock({ $0.failed }) { return "Updates unavailable" }
        return storage.withLock { $0.count > 0 } ? "Receiving at requested 100 Hz" : "Waiting for samples"
    }

    func start() {
        guard !manager.isDeviceMotionActive, manager.isDeviceMotionAvailable,
              CMMotionManager.availableAttitudeReferenceFrames().contains(.xArbitraryCorrectedZVertical) else {
            storage.withLock { $0.failed = true }
            return
        }
        manager.deviceMotionUpdateInterval = 1 / 100
        let generation = storage.withLock { $0.generation }
        // CoreMotion invokes this on the serial operation queue, not MainActor.
        manager.startDeviceMotionUpdates(using: .xArbitraryCorrectedZVertical, to: queue) { @Sendable [storage] motion, error in
            let sample = motion.map { MotionSample($0) }
            let failed = error != nil
            storage.withLock { buffer in
                guard buffer.generation == generation else { return }
                if let sample { buffer.append(sample) }
                if failed { buffer.failed = true }
            }
        }
    }

    func stop() {
        manager.stopDeviceMotionUpdates()
        storage.withLock { $0 = Buffer() }
    }

    func snapshot(start: Double, end: Double) -> [MotionSample]? {
        let samples = storage.withLock { $0.failed ? [] : $0.samples() }
        guard let first = samples.lastIndex(where: { $0.timestamp <= start - 0.05 }),
              let last = samples.firstIndex(where: { $0.timestamp >= end + 0.05 }), first < last else { return nil }
        let result = samples[first...last]
        // Gross clock mismatches and dropped callbacks must not masquerade as synchronized evidence.
        guard result.allSatisfy({ $0.encoded.allSatisfy(\.isFinite) }),
              zip(result, result.dropFirst()).allSatisfy({ (a, b) in b.timestamp - a.timestamp <= 0.025 }) else { return nil }
        return Array(result)
    }
}
