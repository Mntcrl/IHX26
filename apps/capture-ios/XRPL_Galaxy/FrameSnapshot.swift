import ARKit
import CoreVideo
import Foundation
import simd

nonisolated enum CaptureIssue: UInt32, CaseIterable, Sendable {
    case missingFrame = 1, tracking = 2, missingDepth = 4, imageFormat = 8
    case noMotion = 16, motionUnavailable = 32, cameraUnavailable = 64
    case noUsableDepth = 128, excessRotation = 256

    var message: String {
        switch self {
        case .missingFrame: "No camera frame in a required shutter window."
        case .tracking: "ARKit tracking was not normal in all three selected frames."
        case .missingDepth: "A selected frame lacked usable 256×192 LiDAR depth and confidence planes."
        case .imageFormat: "A selected frame did not provide the 1920×1440 full-range 420f format."
        case .noMotion: "Camera displacement fell below the motion floor between the center and a neighboring frame."
        case .motionUnavailable: "The synchronized 100 Hz CoreMotion window was unavailable or incomplete."
        case .cameraUnavailable: "The camera or unsmoothed LiDAR capture was unavailable."
        case .noUsableDepth: "The center frame had no confident LiDAR depth samples in the policy range."
        case .excessRotation: "A neighboring frame exceeded the capture’s relative rotation limit."
        }
    }

    /// Compact label for the capture screen; full messages stay in the record and the label sidecar.
    var short: String {
        switch self {
        case .missingFrame: "no frame"
        case .tracking: "tracking"
        case .missingDepth: "no depth"
        case .imageFormat: "format"
        case .noMotion: "no motion"
        case .motionUnavailable: "no IMU"
        case .cameraUnavailable: "camera"
        case .noUsableDepth: "depth range"
        case .excessRotation: "rotation"
        }
    }

    static func short(for reason: String) -> String {
        if let issue = allCases.first(where: { $0.message == reason }) { return issue.short }
        if reason.hasPrefix("No frame") { return "no frame" }
        if reason.hasPrefix("Acquisition was interrupted") { return "interrupted" }
        return "error"
    }
}

nonisolated struct FrameSnapshot: Sendable {
    static let width = 1920
    static let height = 1440
    static let depthWidth = 256
    static let depthHeight = 192

    let timestamp: Double
    let arrivalLag: Double
    let intrinsics: simd_float3x3
    let transform: simd_float4x4
    let tracking: UInt8
    let trackingReason: String
    let pixels: Pixels?
    let depth: Depth?

    struct Pixels: Sendable {
        let y: Data
        let cbcr: Data
        let matrix: String
    }
    struct Depth: Sendable {
        let meters: Data
        let confidence: Data

        func hasPolicyDepth(_ limits: CapturePolicyLimits) -> Bool {
            let minMm = Double(limits.depthMinMm)
            let maxMm = Double(limits.depthMaxMm)
            return meters.withUnsafeBytes { depth in
                confidence.withUnsafeBytes { (conf: UnsafeRawBufferPointer) in
                    for i in conf.indices where Int64(conf[i]) >= limits.depthConfMin && conf[i] <= 2 {
                        let mm = (Double(depth.loadUnaligned(fromByteOffset: i * 4, as: Float32.self)) * 1_000).rounded()
                        if mm >= minMm && mm <= maxMm { return true }
                    }
                    return false
                }
            }
        }

        func coverage() -> Double {
            // HUD estimate on a fixed grid; the Rust policy uses its own challenge-dependent samples.
            meters.withUnsafeBytes { depth in
                confidence.withUnsafeBytes { (conf: UnsafeRawBufferPointer) in
                    var count = 0
                    for y in stride(from: 0, to: depthHeight, by: 4) {
                        for x in stride(from: 0, to: depthWidth, by: 4) {
                            let i = y * depthWidth + x
                            let d = depth.loadUnaligned(fromByteOffset: i * 4, as: Float32.self)
                            if d >= 0.2 && d <= 5 && (1...2).contains(conf[i]) { count += 1 }
                        }
                    }
                    return Double(count) / Double(depthWidth * depthHeight / 16)
                }
            }
        }
    }

    var position: SIMD3<Float> {
        let t = transform.columns.3
        return SIMD3(t.x, t.y, t.z)
    }

    init(_ frame: ARFrame, arrival: Double) {
        timestamp = frame.timestamp
        arrivalLag = arrival - timestamp
        intrinsics = frame.camera.intrinsics
        transform = frame.camera.transform
        switch frame.camera.trackingState {
        case .normal: (tracking, trackingReason) = (2, "normal")
        case .notAvailable: (tracking, trackingReason) = (0, "not_available")
        case .limited(let reason):
            tracking = 1
            switch reason {
            case .initializing: trackingReason = "initializing"
            case .excessiveMotion: trackingReason = "excessive_motion"
            case .insufficientFeatures: trackingReason = "insufficient_features"
            case .relocalizing: trackingReason = "relocalizing"
            @unknown default: trackingReason = "limited_unknown"
            }
        }
        pixels = Self.copyPixels(frame.capturedImage)
        depth = Self.copyDepth(frame.sceneDepth)
    }

    private static func copyPixels(_ image: CVPixelBuffer) -> Pixels? {
        guard CVPixelBufferGetPixelFormatType(image) == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
              CVPixelBufferGetWidth(image) == width, CVPixelBufferGetHeight(image) == height,
              CVPixelBufferGetPlaneCount(image) == 2,
              CVPixelBufferGetWidthOfPlane(image, 0) == width,
              CVPixelBufferGetHeightOfPlane(image, 0) == height,
              CVPixelBufferGetWidthOfPlane(image, 1) == width / 2,
              CVPixelBufferGetHeightOfPlane(image, 1) == height / 2,
              CVPixelBufferLockBaseAddress(image, .readOnly) == kCVReturnSuccess else { return nil }
        defer { CVPixelBufferUnlockBaseAddress(image, .readOnly) }
        guard let y = copyRows(CVPixelBufferGetBaseAddressOfPlane(image, 0),
                               stride: CVPixelBufferGetBytesPerRowOfPlane(image, 0), rowBytes: width, rows: height),
              let cbcr = copyRows(CVPixelBufferGetBaseAddressOfPlane(image, 1),
                                  stride: CVPixelBufferGetBytesPerRowOfPlane(image, 1),
                                  rowBytes: width, rows: height / 2) else { return nil }
        let matrix = CVBufferCopyAttachment(image, kCVImageBufferYCbCrMatrixKey, nil) as? String ?? "unknown"
        return Pixels(y: y, cbcr: cbcr, matrix: matrix)
    }

    private static func copyDepth(_ depth: ARDepthData?) -> Depth? {
        guard let depth, let confidence = depth.confidenceMap,
              let meters = copyDepthPlane(depth.depthMap, format: kCVPixelFormatType_DepthFloat32, byteWidth: 4),
              let conf = copyDepthPlane(confidence, format: kCVPixelFormatType_OneComponent8, byteWidth: 1),
              conf.allSatisfy({ $0 <= 2 }) else { return nil }
        // iPhone/iPad ARM64 store the Float32 plane in little-endian order, already GXRW v1 order.
        return Depth(meters: meters, confidence: conf)
    }

    private static func copyDepthPlane(_ buffer: CVPixelBuffer, format: OSType, byteWidth: Int) -> Data? {
        guard CVPixelBufferGetPixelFormatType(buffer) == format, !CVPixelBufferIsPlanar(buffer),
              CVPixelBufferGetWidth(buffer) == depthWidth, CVPixelBufferGetHeight(buffer) == depthHeight,
              CVPixelBufferLockBaseAddress(buffer, .readOnly) == kCVReturnSuccess else { return nil }
        defer { CVPixelBufferUnlockBaseAddress(buffer, .readOnly) }
        return copyRows(CVPixelBufferGetBaseAddress(buffer), stride: CVPixelBufferGetBytesPerRow(buffer),
                        rowBytes: depthWidth * byteWidth, rows: depthHeight)
    }

    private static func copyRows(_ source: UnsafeMutableRawPointer?, stride: Int, rowBytes: Int, rows: Int) -> Data? {
        guard let source, stride >= rowBytes else { return nil }
        if stride == rowBytes { return Data(bytes: source, count: rowBytes * rows) }
        var data = Data(count: rowBytes * rows)
        data.withUnsafeMutableBytes { (bytes: UnsafeMutableRawBufferPointer) in
            for row in 0..<rows {
                bytes.baseAddress!.advanced(by: row * rowBytes)
                    .copyMemory(from: source.advanced(by: row * stride), byteCount: rowBytes)
            }
        }
        return data
    }
}
