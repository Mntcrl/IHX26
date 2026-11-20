import CoreVideo
import CryptoKit
import Foundation

nonisolated enum CameraEvidence {
    // Both the copied samples and the read-only attachment metadata are immutable.
    struct Frame: @unchecked Sendable {
        let samples: Data
        let metadata: [String: Any]
    }

    struct VideoFormat: Sendable {
        let deviceType: String
        let cameraPosition: Int
        let framesPerSecond: Int
        let hdrSupported: Bool

        var metadata: [String: Any] {
            let position: String
            switch cameraPosition {
            case 1: position = "back"
            case 2: position = "front"
            default: position = "unspecified"
            }
            return ["deviceType": deviceType, "position": position,
                    "nominalFramesPerSecond": framesPerSecond, "supportsHDR": hdrSupported]
        }
    }

    static func sha256(_ data: Data) -> String {
        SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }

    static func read(_ buffer: CVPixelBuffer, includeAnalysis: Bool = true) throws -> Frame? {
        let format = CVPixelBufferGetPixelFormatType(buffer)
        let bits: Int
        let levels: ClosedRange<Int>
        switch format {
        case kCVPixelFormatType_420YpCbCr8BiPlanarFullRange: (bits, levels) = (8, 0...255)
        case kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange: (bits, levels) = (8, 16...235)
        case kCVPixelFormatType_420YpCbCr10BiPlanarFullRange: (bits, levels) = (10, 0...1023)
        case kCVPixelFormatType_420YpCbCr10BiPlanarVideoRange: (bits, levels) = (10, 64...940)
        default: return nil
        }

        guard CVPixelBufferGetPlaneCount(buffer) == 2,
              CVPixelBufferLockBaseAddress(buffer, .readOnly) == kCVReturnSuccess else {
            throw CocoaError(.fileReadCorruptFile)
        }
        defer { CVPixelBufferUnlockBaseAddress(buffer, .readOnly) }

        // Strip alignment padding only. Keep the two planes in their original
        // orientation, channel order, and sample representation.
        var samples = Data()
        var planes: [[String: Any]] = []
        var noise: [String: Any] = [:]
        let sampleBytes = bits == 8 ? 1 : 2
        for plane in 0..<2 {
            let width = CVPixelBufferGetWidthOfPlane(buffer, plane)
            let height = CVPixelBufferGetHeightOfPlane(buffer, plane)
            let stride = CVPixelBufferGetBytesPerRowOfPlane(buffer, plane)
            let rowBytes = width * sampleBytes * (plane == 0 ? 1 : 2)
            guard let base = CVPixelBufferGetBaseAddressOfPlane(buffer, plane),
                  width > 0, height > 0, rowBytes <= stride else {
                throw CocoaError(.fileReadCorruptFile)
            }
            planes.append(["channels": plane == 0 ? ["Y"] : ["Cb", "Cr"],
                           "offsetBytes": samples.count, "rowBytes": rowBytes,
                           "width": width, "height": height])
            for row in 0..<height {
                samples.append(base.advanced(by: row * stride).assumingMemoryBound(to: UInt8.self), count: rowBytes)
            }
            if plane == 0, includeAnalysis {
                noise = measureLuma(UnsafeRawPointer(base), width: width, height: height, stride: stride,
                                    bits: bits, levels: levels)
            }
        }

        let attachmentKeys = [
            kCVImageBufferYCbCrMatrixKey, kCVImageBufferColorPrimariesKey, kCVImageBufferTransferFunctionKey,
            kCVImageBufferChromaLocationTopFieldKey, kCVImageBufferChromaLocationBottomFieldKey,
            kCVImageBufferCleanApertureKey, kCVImageBufferPixelAspectRatioKey, kCVImageBufferICCProfileKey,
            kCVImageBufferGammaLevelKey
        ]
        var colorMetadata: [String: Any] = [:]
        for key in attachmentKeys {
            if let value = CVBufferCopyAttachment(buffer, key, nil) {
                colorMetadata[key as String] = value
            }
        }

        return Frame(samples: samples, metadata: [
            "source": "ARFrame.capturedImage, Apple camera pipeline output before Fieldnote's JPEG encoding",
            "layout": "headerless row-major planes, no row padding",
            "orientation": "native sensor coordinates, no app rotation",
            "pixelFormatFourCC": fourCC(format),
            "bitsPerComponent": bits,
            "sampleEncoding": bits == 8 ? "UInt8" : "UInt16LE with 10 significant bits in the most significant positions",
            "lumaCodeRange": [levels.lowerBound, levels.upperBound],
            "planes": planes,
            "colorAndSamplingAttachments": colorMetadata,
            "noiseAnalysis": noise
        ])
    }

    static func fourCC(_ format: OSType) -> String {
        let bytes = [24, 16, 8, 0].map { UInt8(truncatingIfNeeded: format >> $0) }
        return String(bytes: bytes, encoding: .ascii) ?? String(format: "0x%08X", format)
    }

    private struct Tile {
        let x: Int
        let y: Int
        let width: Int
        let height: Int
        let mean: Double
        let activity: Double
        let residualRMS: Double
        let clippedFraction: Double
    }

    private static func measureLuma(_ base: UnsafeRawPointer, width: Int, height: Int, stride: Int,
                                    bits: Int, levels: ClosedRange<Int>) -> [String: Any] {
        let tileSize = 64
        var tiles: [Tile] = []
        func luma(_ x: Int, _ y: Int) -> Int {
            let row = base.advanced(by: y * stride)
            if bits == 8 { return Int(row.load(fromByteOffset: x, as: UInt8.self)) }
            let word = row.loadUnaligned(fromByteOffset: x * 2, as: UInt16.self)
            return Int(UInt16(littleEndian: word) >> 6)
        }

        for top in Swift.stride(from: 0, to: height, by: tileSize) {
            for left in Swift.stride(from: 0, to: width, by: tileSize) {
                let bottom = min(height, top + tileSize)
                let right = min(width, left + tileSize)
                var count = 0
                var sum = 0
                var squaredResidual = 0
                var activity = 0
                var clipped = 0
                for y in Swift.stride(from: top, to: bottom - 1, by: 2) {
                    for x in Swift.stride(from: left, to: right - 1, by: 2) {
                        let a = luma(x, y), b = luma(x + 1, y), c = luma(x, y + 1), d = luma(x + 1, y + 1)
                        let residual = a - b - c + d
                        squaredResidual += residual * residual
                        activity += abs(a + c - b - d) + abs(a + b - c - d)
                        sum += a + b + c + d
                        if a <= levels.lowerBound || a >= levels.upperBound { clipped += 1 }
                        if b <= levels.lowerBound || b >= levels.upperBound { clipped += 1 }
                        if c <= levels.lowerBound || c >= levels.upperBound { clipped += 1 }
                        if d <= levels.lowerBound || d >= levels.upperBound { clipped += 1 }
                        count += 1
                    }
                }
                guard count > 0 else { continue }
                let pixels = Double(4 * count)
                tiles.append(Tile(x: left, y: top, width: right - left, height: bottom - top,
                                  mean: Double(sum) / pixels, activity: Double(activity) / pixels,
                                  residualRMS: sqrt(Double(squaredResidual) / pixels),
                                  clippedFraction: Double(clipped) / pixels))
            }
        }

        let eligible = tiles.filter { $0.width == tileSize && $0.height == tileSize && $0.clippedFraction < 0.01 }
            .sorted { $0.activity < $1.activity }
        let lowActivity = eligible.prefix(eligible.isEmpty ? 0 : max(1, eligible.count / 4))
        let estimates = lowActivity.map(\.residualRMS).sorted()
        let median: Any = estimates.isEmpty ? NSNull() : estimates[estimates.count / 2] as Any
        return [
            "method": "2x2 diagonal high-pass residual (a-b-c+d)/2 on native luma; independent nonoverlapping blocks",
            "units": "luma code values",
            "interpretation": "Spatial residual includes scene detail and camera processing. It is not a calibrated sensor noise or device ID measurement.",
            "tileSizePixels": tileSize,
            "summary": ["selection": "lowest directional activity quartile of full tiles with less than 1% clipping",
                        "selectedTileCount": lowActivity.count, "medianResidualRMS": median],
            "tileColumns": ["x", "y", "width", "height", "meanLuma", "directionalActivity", "residualRMS", "clippedFraction"],
            "tiles": tiles.map { [Double($0.x), Double($0.y), Double($0.width), Double($0.height),
                                  $0.mean, $0.activity, $0.residualRMS, $0.clippedFraction] }
        ]
    }
}
