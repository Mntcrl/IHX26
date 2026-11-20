import ARKit
import Foundation
import simd

nonisolated struct SpatialEvidenceCollector {
    struct Pose: Sendable {
        let time: Double
        let matrix: simd_float4x4
        let trackingNormal: Bool
    }

    static func pose(_ frame: ARFrame) -> Pose {
        Pose(time: frame.timestamp, matrix: frame.camera.transform, trackingNormal: frame.camera.trackingState == .normal)
    }

    static func collect(frame: ARFrame, poses: [Pose], center: Double, clock: String) -> [String: Any] {
        let trajectory: [[String: Any]] = poses.map {
            let p = $0.matrix.columns.3
            let q = simd_quatf($0.matrix).vector
            return ["offsetMilliseconds": milliseconds($0.time - center), "positionMeters": [p.x, p.y, p.z],
                    "quaternionXYZW": [q.x, q.y, q.z, q.w], "normalTracking": $0.trackingNormal]
        }
        let camera = frame.camera
        let inverse = camera.transform.inverse
        let meshFaces = frame.anchors.compactMap { $0 as? ARMeshAnchor }.reduce(0) { $0 + $1.geometry.faces.count }
        let points = frame.rawFeaturePoints?.points ?? []
        let forward = points.map { (inverse * SIMD4($0.x, $0.y, $0.z, 1)).z }.filter { $0.isFinite && $0 < 0 }.count
        var jumps = 0
        var distance: Float = 0
        for (a, b) in zip(poses, poses.dropFirst()) where a.trackingNormal && b.trackingNormal && b.time > a.time && b.time - a.time < 0.5 {
            let delta = b.matrix.columns.3 - a.matrix.columns.3
            let length = simd_length(SIMD3(delta.x, delta.y, delta.z))
            if length.isFinite {
                distance += length
                if Double(length) / (b.time - a.time) > 5 { jumps += 1 }
            }
        }
        let depth = depthGeometry(frame, features: points)
        let normalCount = poses.filter(\.trackingNormal).count
        let coverage = depth["validDepthRatio"] as? Double ?? 0
        let comparisons = depth["featureDepthComparisons"] as? Int ?? 0
        let residual = depth["medianFeatureDepthRelativeError"] as? Double
        let status: String
        if coverage < 0.35 || normalCount < 3 || comparisons < 8 { status = "insufficientData" }
        else if jumps > 0 { status = "motionDiscontinuity" }
        else if (residual ?? .infinity) > 0.20 { status = "depthFeatureDisagreement" }
        else { status = "passed" }
        return [
            "clock": clock, "cameraFrameUptimeMilliseconds": milliseconds(frame.timestamp),
            "coordinateFrame": "ARKit world, meters; local depth rays use camera x right, y up, forward -z",
            "trajectory": trajectory, "surfaceSamples": depth,
            "consistency": ["status": status,
                            "normalTrackingSamples": normalCount, "motionDiscontinuities": jumps,
                            "trackedPathMeters": distance, "reconstructedTriangleCount": meshFaces,
                            "trackedFeaturesInFrontOfCamera": forward,
                            "criterion": "At least 35% valid depth, 3 normal poses, 8 feature and depth comparisons, at most 20% median relative error, and no adjacent normal pose above 5 m/s. Heuristic only."]
        ]
    }

    private static func milliseconds(_ seconds: Double) -> Any {
        let value = (seconds * 1000).rounded()
        return Int64(exactly: value) as Any? ?? NSNull()
    }

    private static func depthGeometry(_ frame: ARFrame, features: [SIMD3<Float>]) -> [String: Any] {
        guard let scene = frame.sceneDepth, CVPixelBufferGetPixelFormatType(scene.depthMap) == kCVPixelFormatType_DepthFloat32 else { return [:] }
        let depth = scene.depthMap
        guard CVPixelBufferLockBaseAddress(depth, .readOnly) == kCVReturnSuccess else { return [:] }
        defer { CVPixelBufferUnlockBaseAddress(depth, .readOnly) }
        guard let base = CVPixelBufferGetBaseAddress(depth) else { return [:] }
        let confidence = scene.confidenceMap
        let locked = confidence.map { CVPixelBufferLockBaseAddress($0, .readOnly) == kCVReturnSuccess } ?? false
        defer { if let confidence, locked { CVPixelBufferUnlockBaseAddress(confidence, .readOnly) } }
        let width = CVPixelBufferGetWidth(depth), height = CVPixelBufferGetHeight(depth)
        let stride = CVPixelBufferGetBytesPerRow(depth)
        let confidenceBase: UnsafeMutableRawPointer?
        if let confidence, locked, CVPixelBufferGetWidth(confidence) == width, CVPixelBufferGetHeight(confidence) == height,
           CVPixelBufferGetPixelFormatType(confidence) == kCVPixelFormatType_OneComponent8 {
            confidenceBase = CVPixelBufferGetBaseAddress(confidence)
        } else { confidenceBase = nil }
        let confidenceStride = confidence.map(CVPixelBufferGetBytesPerRow) ?? 0
        let camera = frame.camera
        let k = camera.intrinsics
        let sx = Float(camera.imageResolution.width) / Float(width)
        let sy = Float(camera.imageResolution.height) / Float(height)
        guard width > 0, height > 0, k.columns.0.x > 0, k.columns.1.y > 0 else { return [:] }

        func sample(_ x: Int, _ y: Int) -> SIMD3<Float>? {
            let value = base.loadUnaligned(fromByteOffset: y * stride + x * 4, as: Float.self)
            guard value.isFinite, value > 0 else { return nil }
            let px = (Float(x) + 0.5) * sx - 0.5
            let py = (Float(y) + 0.5) * sy - 0.5
            return SIMD3((px - k.columns.2.x) / k.columns.0.x * value, (k.columns.2.y - py) / k.columns.1.y * value, -value)
        }

        var valid = 0
        var vectors: [[String: Any]] = []
        for y in 0..<height {
            for x in 0..<width {
                guard let p = sample(x, y) else { continue }
                valid += 1
                guard x % 8 == 0, y % 8 == 0 else { continue }
                let w = camera.transform * SIMD4(p.x, p.y, p.z, 1)
                let direction = simd_normalize(p)
                var entry: [String: Any] = ["depthPixel": [x, y], "cameraRayXYZ": [direction.x, direction.y, direction.z],
                                            "worldPointMeters": [w.x, w.y, w.z], "axialDepthMeters": -p.z]
                if let confidenceBase { entry["confidence"] = confidenceBase.load(fromByteOffset: y * confidenceStride + x, as: UInt8.self) }
                if x > 0, x + 1 < width, y > 0, y + 1 < height,
                   let left = sample(x-1, y), let right = sample(x+1, y),
                   let top = sample(x, y-1), let bottom = sample(x, y+1),
                   [left, right, top, bottom].allSatisfy({ abs($0.z - p.z) < max(0.03, -p.z * 0.04) }) {
                    let n = simd_cross(bottom - top, right - left)
                    if simd_length_squared(n) > 1e-12 {
                        let unit = simd_normalize(n)
                        let wn = camera.transform * SIMD4(unit.x, unit.y, unit.z, 0)
                        entry["worldNormalXYZ"] = [wn.x, wn.y, wn.z]
                    }
                }
                vectors.append(entry)
            }
        }
        let inverse = camera.transform.inverse
        var compared = Set<Int>()
        var errors: [Double] = []
        for point in features {
            let p = inverse * SIMD4(point.x, point.y, point.z, 1)
            let z = -p.z
            guard z.isFinite, z > 0 else { continue }
            let x = (p.x / z * k.columns.0.x + k.columns.2.x + 0.5) / sx
            let y = (-p.y / z * k.columns.1.y + k.columns.2.y + 0.5) / sy
            guard x.isFinite, y.isFinite, x >= 0, x < Float(width), y >= 0, y < Float(height) else { continue }
            let ix = Int(x), iy = Int(y)
            guard !compared.contains(iy * width + ix), let measured = sample(ix, iy) else { continue }
            if let confidenceBase, confidenceBase.load(fromByteOffset: iy * confidenceStride + ix, as: UInt8.self) < 1 { continue }
            compared.insert(iy * width + ix)
            errors.append(Double(abs(z + measured.z) / -measured.z))
        }
        errors.sort()
        let median: Any = errors.isEmpty ? NSNull() : errors[errors.count / 2] as Any
        return ["depthResolution": [width, height], "sampleStridePixels": 8,
                "validDepthRatio": Double(valid) / Double(width * height), "vectors": vectors,
                "featureDepthComparisons": errors.count, "medianFeatureDepthRelativeError": median]
    }
}
