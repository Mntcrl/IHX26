import Foundation
import simd

/// Scene classes of the physical-device experiment protocol
/// (docs/experiments/device-capture-protocol.md, user brief §23.3). The label is chosen by the
/// photographer before pressing the shutter and is written only to a sidecar file next to the
/// exported fixture; it is never part of the GXRW bytes or of any commitment.
nonisolated enum SceneLabel: String, CaseIterable, Codable, Identifiable, Sendable {
    case unlabelled = "c00-unlabelled"
    case indoor3D = "c01-indoor-3d"
    case outdoor = "c02-outdoor"
    case desktopMonitor = "c03-desktop-monitor"
    case laptopScreen = "c04-laptop-screen"
    case phoneScreen = "c05-phone-screen"
    case printedPhoto = "c06-printed-photo"
    case projectedImage = "c07-projected-image"
    case tiltedScreen = "c08-tilted-screen"
    case screenWithObjects = "c09-screen-with-objects"
    case flatWall = "c10-flat-wall"
    case reflective = "c11-reflective"
    case lowTexture = "c12-low-texture"
    case lowMotion = "c13-low-motion"
    case dynamicScene = "c14-dynamic"

    var id: String { rawValue }

    var title: String {
        switch self {
        case .unlabelled: "Unlabelled"
        case .indoor3D: "1 · 3-D indoor scene"
        case .outdoor: "2 · Outdoor scene"
        case .desktopMonitor: "3 · Desktop monitor"
        case .laptopScreen: "4 · Laptop screen"
        case .phoneScreen: "5 · Phone screen"
        case .printedPhoto: "6 · Printed photograph"
        case .projectedImage: "7 · Projected image"
        case .tiltedScreen: "8 · Tilted screen"
        case .screenWithObjects: "9 · Screen among objects"
        case .flatWall: "10 · Genuine flat wall"
        case .reflective: "11 · Reflective surfaces"
        case .lowTexture: "12 · Low-texture scene"
        case .lowMotion: "13 · Low-motion capture"
        case .dynamicScene: "14 · Dynamic scene"
        }
    }
}

/// Motion summary of one attempt, from the ARKit camera poses of the selected frames.
nonisolated struct MotionSummary: Codable, Sendable {
    var baselineMinusMm: Double?
    var baselinePlusMm: Double?
    var rotationMinusDeg: Double?
    var rotationPlusDeg: Double?

    enum CodingKeys: String, CodingKey {
        case baselineMinusMm = "baseline_minus_mm"
        case baselinePlusMm = "baseline_plus_mm"
        case rotationMinusDeg = "rotation_minus_deg"
        case rotationPlusDeg = "rotation_plus_deg"
    }
}

/// Sidecar written for EVERY attempt (`Documents/galaxy-fixtures/<uuid>.label.json`), including
/// attempts without a raw capture, so that experiment statistics count every attempt.
/// Contains no raw sensor values: only the label, the app's decision and coarse motion/depth summaries.
nonisolated struct CaptureLabel: Encodable, Sendable {
    let format = "galaxy-capture-label/v1"
    let captureId: UUID
    let sceneClass: SceneLabel
    let claimedTimeMs: UInt64
    let state: CaptureState
    let reasons: [String]
    let rawExported: Bool
    let motion: MotionSummary
    let depthCoverage: Double?
    let appBuild: String

    enum CodingKeys: String, CodingKey {
        case format
        case captureId = "capture_id"
        case sceneClass = "scene_class"
        case claimedTimeMs = "claimed_time_ms"
        case state, reasons
        case rawExported = "raw_exported"
        case motion
        case depthCoverage = "depth_coverage"
        case appBuild = "app_build"
    }
}

nonisolated enum PoseMath {
    /// Camera-center distance in millimetres between two camera→world transforms.
    static func baselineMm(_ a: simd_float4x4, _ b: simd_float4x4) -> Double {
        let d = a.columns.3 - b.columns.3
        return Double(simd_length(SIMD3(d.x, d.y, d.z))) * 1000
    }

    /// Relative rotation angle in degrees between two camera→world transforms.
    static func rotationDeg(_ a: simd_float4x4, _ b: simd_float4x4) -> Double {
        let ra = simd_float3x3(SIMD3(a.columns.0.x, a.columns.0.y, a.columns.0.z),
                               SIMD3(a.columns.1.x, a.columns.1.y, a.columns.1.z),
                               SIMD3(a.columns.2.x, a.columns.2.y, a.columns.2.z))
        let rb = simd_float3x3(SIMD3(b.columns.0.x, b.columns.0.y, b.columns.0.z),
                               SIMD3(b.columns.1.x, b.columns.1.y, b.columns.1.z),
                               SIMD3(b.columns.2.x, b.columns.2.y, b.columns.2.z))
        let r = ra.transpose * rb
        let trace = Double(r.columns.0.x + r.columns.1.y + r.columns.2.z)
        let cosine = min(1, max(-1, (trace - 1) / 2))
        return acos(cosine) * 180 / .pi
    }
}
