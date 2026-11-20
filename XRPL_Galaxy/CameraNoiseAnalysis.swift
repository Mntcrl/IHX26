import ARKit
import Foundation
import simd

nonisolated enum CameraNoiseAnalysis {
    private struct Channel {
        let name: String
        let width: Int
        let height: Int
        let sampleBytes: Int
        let components: Int
        let inputOffset: Int
        let rowBytes: Int

        @inline(__always) func code(_ base: UnsafeRawPointer, _ x: Int, _ y: Int) -> Int {
            let offset = inputOffset + y * rowBytes + x * sampleBytes * components
            if sampleBytes == 1 { return Int(base.load(fromByteOffset: offset, as: UInt8.self)) }
            return Int(UInt16(littleEndian: base.loadUnaligned(fromByteOffset: offset, as: UInt16.self)) >> 6)
        }
    }

    struct Spatial: Sendable {
        let samples: Data
        fileprivate let channelMetadata: Data
    }

    static func prepareSpatial(_ native: CameraEvidence.Frame) throws -> Spatial {
        guard let channels = layout(native) else { throw CocoaError(.fileReadCorruptFile) }
        return try native.samples.withUnsafeBytes { pixels in
            guard let base = pixels.baseAddress else { throw CocoaError(.fileReadCorruptFile) }
            var output = Data(capacity: channels.reduce(0) { $0 + $1.width * $1.height * 2 })
            var metadata: [[String: Any]] = []
            for channel in channels {
                let result = spatialResidual(channel, base: base, offset: output.count)
                result.samples.withUnsafeBytes { output.append(contentsOf: $0) }
                metadata.append(result.metadata)
            }
            return Spatial(samples: output, channelMetadata: try ScanExport.encodeJSON(["channels": metadata]))
        }
    }

    static func metadata(_ spatial: Spatial, native: CameraEvidence.Frame, frame: ARFrame,
                         references: [ARFrame], fileURL: URL) throws -> Data {
        guard let channels = layout(native),
              let object = try JSONSerialization.jsonObject(with: spatial.channelMetadata) as? [String: Any],
              let spatialChannels = object["channels"] else { throw CocoaError(.fileReadCorruptFile) }
        let temporal = references.map { temporalComparison($0, against: frame, native: native, channels: channels) }
        let sourceFile = fileURL.deletingPathExtension().deletingPathExtension().appendingPathExtension("yuv")
        return try ScanExport.encodeJSON([
            "schemaVersion": 1,
            "sourceNativeFrame": sourceFile.lastPathComponent,
            "pixelFormatFourCC": native.metadata["pixelFormatFourCC"] ?? NSNull(),
            "captureUptimeSeconds": frame.timestamp,
            "sourceExposureDurationSeconds": frame.camera.exposureDuration,
            "sourceExposureOffsetEV": frame.camera.exposureOffset,
            "residualFile": fileURL.lastPathComponent,
            "layout": "three native-resolution planes Y, Cb, Cr; signed Int16LE, row-major with no padding",
            "formula": "4 * center - left - right - up - down; clamp neighbor coordinates at image boundaries",
            "spatialChannels": spatialChannels,
            "temporalStatus": temporal.isEmpty ? "noReferenceFrames" : "referenceFramesEvaluated",
            "temporalComparisons": temporal,
            "interpretation": "Observed post-ISP spatial and temporal residuals include scene detail, movement, and camera processing. They do not isolate silicon noise or authenticate a sensor."
        ])
    }

    private static func layout(_ frame: CameraEvidence.Frame) -> [Channel]? {
        guard let bits = frame.metadata["bitsPerComponent"] as? Int, bits == 8 || bits == 10,
              let planes = frame.metadata["planes"] as? [[String: Any]], planes.count == 2 else { return nil }
        let bytes = bits == 8 ? 1 : 2
        var channels: [Channel] = []
        for plane in planes {
            guard let names = plane["channels"] as? [String], [1, 2].contains(names.count),
                  let offset = plane["offsetBytes"] as? Int, offset >= 0,
                  let width = plane["width"] as? Int, width > 0,
                  let height = plane["height"] as? Int, height > 0,
                  let rowBytes = plane["rowBytes"] as? Int, rowBytes >= width * names.count * bytes,
                  offset + rowBytes * height <= frame.samples.count else { return nil }
            for (index, name) in names.enumerated() {
                channels.append(Channel(name: name, width: width, height: height, sampleBytes: bytes,
                                        components: names.count, inputOffset: offset + index * bytes, rowBytes: rowBytes))
            }
        }
        return channels.map(\.name) == ["Y", "Cb", "Cr"] ? channels : nil
    }

    private static func spatialResidual(_ channel: Channel, base: UnsafeRawPointer,
                                        offset: Int) -> (samples: [Int16], metadata: [String: Any]) {
        let width = channel.width, height = channel.height
        var values = Array(repeating: Int16.zero, count: width * height)
        var rows = Array(repeating: Double.zero, count: height)
        var columns = Array(repeating: Int.zero, count: width)
        var minimum = Int.max, maximum = Int.min, squared: Int64 = 0
        for y in 0..<height {
            let above = max(0, y - 1), below = min(height - 1, y + 1)
            var rowSum = 0
            for x in 0..<width {
                let r = 4 * channel.code(base, x, y) - channel.code(base, max(0, x - 1), y)
                    - channel.code(base, min(width - 1, x + 1), y)
                    - channel.code(base, x, above) - channel.code(base, x, below)
                values[y * width + x] = Int16(r).littleEndian
                rowSum += r
                columns[x] += r
                squared += Int64(r * r)
                minimum = min(minimum, r)
                maximum = max(maximum, r)
            }
            rows[y] = Double(rowSum) / Double(width)
        }
        let metadata: [String: Any] = [
            "channel": channel.name, "width": width, "height": height, "offsetBytes": offset, "rowBytes": width * 2,
            "minimum": minimum, "maximum": maximum, "rms": sqrt(Double(squared) / Double(values.count)),
            "rowResidualMean": rows, "columnResidualMean": columns.map { Double($0) / Double(height) }
        ]
        return (values, metadata)
    }

    private static func temporalComparison(_ reference: ARFrame, against frame: ARFrame, native: CameraEvidence.Frame,
                                           channels: [Channel]) -> [String: Any] {
        let a = simd_quatf(frame.camera.transform), b = simd_quatf(reference.camera.transform)
        let angle = Double(2 * acos(min(Float(1), abs(simd_dot(a.vector, b.vector))))) * 180 / .pi
        let position = simd_distance(frame.camera.transform.columns.3, reference.camera.transform.columns.3)
        let duration = frame.camera.exposureDuration
        let exposureRatio = duration > 0 ? reference.camera.exposureDuration / duration : 0
        let deltaEV = Double(reference.camera.exposureOffset - frame.camera.exposureOffset)
        let sourceK = frame.camera.intrinsics, refK = reference.camera.intrinsics
        let calibrationDelta = max(simd_distance(sourceK.columns.0, refK.columns.0),
                                   simd_distance(sourceK.columns.1, refK.columns.1),
                                   simd_distance(sourceK.columns.2, refK.columns.2))
        var report: [String: Any] = [
            "referenceOffsetSeconds": reference.timestamp - frame.timestamp,
            "translationMeters": position, "rotationDegrees": angle, "intrinsicsDeltaPixels": calibrationDelta,
            "referenceExposureDurationSeconds": reference.camera.exposureDuration,
            "relativeExposureDuration": exposureRatio, "exposureOffsetDifferenceEV": deltaEV
        ]
        guard angle.isFinite, position.isFinite, calibrationDelta.isFinite,
              angle <= 0.2, position <= 0.004, calibrationDelta <= 1,
              reference.camera.trackingState == .normal, frame.camera.trackingState == .normal else {
            report["status"] = "cameraMotionOrCalibrationChange"
            return report
        }
        guard (0.97...1.03).contains(exposureRatio), abs(deltaEV) <= 0.15 else {
            report["status"] = "exposureChanged"
            return report
        }
        guard let other = try? CameraEvidence.read(reference.capturedImage, includeAnalysis: false),
              other.metadata["pixelFormatFourCC"] as? String == native.metadata["pixelFormatFourCC"] as? String,
              let otherChannels = layout(other), otherChannels.count == channels.count,
              zip(channels, otherChannels).allSatisfy({ $0.width == $1.width && $0.height == $1.height }) else {
            report["status"] = "pixelLayoutsDoNotMatch"
            return report
        }

        report["status"] = "measured"
        report["method"] = "reference minus capture at matching native sample coordinates; every second sample in each axis; no resampling"
        report["tileColumns"] = ["x", "y", "width", "height", "meanDifference", "differenceRMS"]
        report["channels"] = native.samples.withUnsafeBytes { captured in
            other.samples.withUnsafeBytes { compared -> [[String: Any]] in
                guard let original = captured.baseAddress, let reference = compared.baseAddress else { return [] }
                return zip(channels, otherChannels).map { a, b in
                    var tiles: [[Double]] = []
                    for y0 in stride(from: 0, to: a.height, by: 64) {
                        for x0 in stride(from: 0, to: a.width, by: 64) {
                            let endX = min(a.width, x0 + 64), endY = min(a.height, y0 + 64)
                            var sum = 0, squares = 0, count = 0
                            for y in stride(from: y0, to: endY, by: 2) {
                                for x in stride(from: x0, to: endX, by: 2) {
                                    let diff = b.code(reference, x, y) - a.code(original, x, y)
                                    sum += diff
                                    squares += diff * diff
                                    count += 1
                                }
                            }
                            tiles.append([Double(x0), Double(y0), Double(endX - x0), Double(endY - y0),
                                          Double(sum) / Double(count), sqrt(Double(squares) / Double(count))])
                        }
                    }
                    return ["channel": a.name, "tiles": tiles]
                }
            }
        }
        return report
    }
}
