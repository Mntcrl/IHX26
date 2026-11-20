import ARKit
import Foundation

nonisolated enum PhysicsBurst {
    static let suffixes = ["physics.json", "ref0.y", "ref1.y", "ref0.fdepth", "ref1.fdepth"]

    static func write(shutter: ARFrame, references: [ARFrame], center: Double,
                      sourcePhoto: URL, depthData: (CVPixelBuffer) -> Data?) throws -> [String: String] {
        let prefix = sourcePhoto.deletingPathExtension()
        var hashes: [String: String] = [:]
        var entries: [[String: Any]] = []
        func store(_ data: Data, suffix: String) throws -> String {
            let url = prefix.appendingPathExtension(suffix)
            try data.write(to: url, options: .atomic)
            hashes[url.lastPathComponent] = CameraEvidence.sha256(data)
            return url.lastPathComponent
        }
        for (index, frame) in references.prefix(2).enumerated() {
            guard let luma = copyLuma(frame.capturedImage) else { continue }
            var entry: [String: Any] = [
                "lumaFile": try store(luma.bytes, suffix: "ref\(index).y"),
                "width": luma.width, "height": luma.height, "bitsPerComponent": luma.bits,
                "intrinsics": intrinsics(frame.camera),
                "offsetSecondsFromCaptureWindowCenter": frame.timestamp - center,
                "offsetSecondsFromShutterFrame": frame.timestamp - shutter.timestamp
            ]
            if let scene = frame.sceneDepth, let packed = depthData(scene.depthMap) {
                entry["depthFile"] = try store(packed, suffix: "ref\(index).fdepth")
            }
            entries.append(entry)
        }
        let metadata: [String: Any] = [
            "schemaVersion": 1,
            "sampleLayout": "Native uncropped camera luma, tightly packed rows. Eight-bit unsigned or 10-bit in the high bits of UInt16LE.",
            "shutterManifest": prefix.appendingPathExtension("json").lastPathComponent,
            "imuFile": prefix.appendingPathExtension("imu.json").lastPathComponent,
            "shutterOffsetSecondsFromCaptureWindowCenter": shutter.timestamp - center,
            "references": entries
        ]
        _ = try store(ScanExport.encodeJSON(metadata), suffix: "physics.json")
        return hashes
    }

    private static func intrinsics(_ camera: ARCamera) -> [[Float]] {
        let k = camera.intrinsics
        return [[k.columns.0.x, 0, k.columns.2.x], [0, k.columns.1.y, k.columns.2.y], [0, 0, 1]]
    }

    private static func copyLuma(_ buffer: CVPixelBuffer) -> (bytes: Data, width: Int, height: Int, bits: Int)? {
        let bits: Int
        switch CVPixelBufferGetPixelFormatType(buffer) {
        case kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange: bits = 8
        case kCVPixelFormatType_420YpCbCr10BiPlanarVideoRange, kCVPixelFormatType_420YpCbCr10BiPlanarFullRange: bits = 10
        default: return nil
        }
        guard CVPixelBufferGetPlaneCount(buffer) == 2,
              CVPixelBufferLockBaseAddress(buffer, .readOnly) == kCVReturnSuccess else { return nil }
        defer { CVPixelBufferUnlockBaseAddress(buffer, .readOnly) }
        let width = CVPixelBufferGetWidthOfPlane(buffer, 0), height = CVPixelBufferGetHeightOfPlane(buffer, 0)
        let stride = CVPixelBufferGetBytesPerRowOfPlane(buffer, 0)
        let rowBytes = width * (bits == 8 ? 1 : 2)
        guard width > 0, height > 0, rowBytes <= stride,
              let base = CVPixelBufferGetBaseAddressOfPlane(buffer, 0) else { return nil }
        var data = Data(capacity: rowBytes * height)
        for y in 0..<height {
            data.append(base.advanced(by: y * stride).assumingMemoryBound(to: UInt8.self), count: rowBytes)
        }
        return (data, width, height, bits)
    }
}
