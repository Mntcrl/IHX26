import ARKit
import SwiftUI
import simd

/// Live visual diagnostics drawn over the camera preview: the 8×6 commitment tile grid, LiDAR depth
/// samples, rays from the principal point, a colorized depth map, a horizon/level line and a top-down
/// motion trail. Everything is derived from the frames the acquisition pipeline already copies; the
/// overlay never feeds back into the committed data.
nonisolated struct OverlayDot: Sendable {
    let point: CGPoint      // normalized captured-image coordinates (0...1, origin top-left)
    let depth: Float        // metres
    let confidence: UInt8   // ARConfidenceLevel 0...2
}

nonisolated enum Turbo {
    /// Google Turbo colormap (polynomial approximation), 256 entries of RGB in 0...1.
    static let table: [SIMD3<Float>] = (0..<256).map { i in
        let t = Float(i) / 255
        func poly(_ c: [Float]) -> Float {
            var v: Float = 0
            for k in c.reversed() { v = v * t + k }
            return min(1, max(0, v))
        }
        return SIMD3(poly([0.13572138, 4.61539260, -42.66032258, 132.13108234, -152.94239396, 59.28637943]),
                     poly([0.09140261, 2.19418839, 4.84296658, -14.18503333, 4.27729857, 2.82956604]),
                     poly([0.10667330, 12.64194608, -60.58204836, 110.36276771, -89.90310912, 27.34824973]))
    }

    /// Log-scaled depth index for 0.2...5 m.
    static func index(_ depth: Float) -> Int {
        let t = (log(max(0.2, min(5, depth))) - log(Float(0.2))) / (log(Float(5)) - log(Float(0.2)))
        return Int((t * 255).rounded())
    }

    static func color(_ depth: Float) -> Color {
        let c = table[index(depth)]
        return Color(red: Double(c.x), green: Double(c.y), blue: Double(c.z))
    }
}

@MainActor
@Observable
final class OverlayModel {
    private(set) var displayTransform: CGAffineTransform = .identity
    private(set) var dots: [OverlayDot] = []
    private(set) var depthImage: CGImage?
    private(set) var principal = CGPoint(x: 0.5, y: 0.5)
    /// Image-plane "up" (world up projected), in captured-image pixels, unit length.
    private(set) var upPixels = CGVector(dx: 0, dy: -1)
    /// Recent camera positions in the camera's horizontal frame, millimetres, relative to the latest.
    private(set) var trail: [CGPoint] = []
    private(set) var framesPerSecond: Double = 0

    var showGrid = UserDefaults.standard.object(forKey: "overlay.grid") as? Bool ?? true {
        didSet { UserDefaults.standard.set(showGrid, forKey: "overlay.grid") }
    }
    var showDots = UserDefaults.standard.object(forKey: "overlay.dots") as? Bool ?? true {
        didSet { UserDefaults.standard.set(showDots, forKey: "overlay.dots") }
    }
    var showRays = UserDefaults.standard.object(forKey: "overlay.rays") as? Bool ?? false {
        didSet { UserDefaults.standard.set(showRays, forKey: "overlay.rays") }
    }
    var showDepth = UserDefaults.standard.object(forKey: "overlay.depth") as? Bool ?? false {
        didSet { UserDefaults.standard.set(showDepth, forKey: "overlay.depth") }
    }
    var showLevel = UserDefaults.standard.object(forKey: "overlay.level") as? Bool ?? true {
        didSet { UserDefaults.standard.set(showLevel, forKey: "overlay.level") }
    }
    var showTrail = UserDefaults.standard.object(forKey: "overlay.trail") as? Bool ?? true {
        didSet { UserDefaults.standard.set(showTrail, forKey: "overlay.trail") }
    }
    var showFeatures = UserDefaults.standard.object(forKey: "overlay.features") as? Bool ?? false {
        didSet { UserDefaults.standard.set(showFeatures, forKey: "overlay.features") }
    }

    @ObservationIgnored private var positions: [(time: Double, position: SIMD3<Float>)] = []
    @ObservationIgnored private var lastDepthImage: Double = 0
    @ObservationIgnored private var lastTimestamp: Double?

    func reset() {
        dots = []
        depthImage = nil
        trail = []
        positions = []
        lastTimestamp = nil
        framesPerSecond = 0
    }

    func update(frame: ARFrame, snapshot: FrameSnapshot, viewport: CGSize, orientation: UIInterfaceOrientation) {
        guard viewport.width > 0, viewport.height > 0 else { return }
        displayTransform = frame.displayTransform(for: orientation, viewportSize: viewport)
        if let previous = lastTimestamp, snapshot.timestamp > previous {
            let instant = 1 / (snapshot.timestamp - previous)
            framesPerSecond = framesPerSecond == 0 ? instant : framesPerSecond * 0.8 + instant * 0.2
        }
        lastTimestamp = snapshot.timestamp

        let k = snapshot.intrinsics
        principal = CGPoint(x: CGFloat(k[2][0]) / CGFloat(FrameSnapshot.width),
                            y: CGFloat(k[2][1]) / CGFloat(FrameSnapshot.height))
        let t = snapshot.transform
        let rotation = simd_float3x3(SIMD3(t.columns.0.x, t.columns.0.y, t.columns.0.z),
                                     SIMD3(t.columns.1.x, t.columns.1.y, t.columns.1.z),
                                     SIMD3(t.columns.2.x, t.columns.2.y, t.columns.2.z))
        // ARKit camera axes: x right, y up, z backward; image pixels: x right, y down.
        let upCamera = rotation.transpose * SIMD3<Float>(0, 1, 0)
        let length = max(1e-6, hypot(upCamera.x, upCamera.y))
        upPixels = CGVector(dx: CGFloat(upCamera.x / length), dy: CGFloat(-upCamera.y / length))

        if let depth = snapshot.depth, showDots || showRays || showDepth {
            dots = Self.sampleDots(depth)
            if showDepth, snapshot.timestamp - lastDepthImage >= 0.1 {
                depthImage = Self.colorize(depth)
                lastDepthImage = snapshot.timestamp
            }
        } else {
            dots = []
        }
        if !showDepth { depthImage = nil }

        positions.append((snapshot.timestamp, snapshot.position))
        positions.removeAll { snapshot.timestamp - $0.time > 2.0 }
        // Plot frame: camera right and forward projected on the horizontal plane (world y is up).
        var right = SIMD3<Float>(rotation.columns.0.x, 0, rotation.columns.0.z)
        var forward = SIMD3<Float>(-rotation.columns.2.x, 0, -rotation.columns.2.z)
        if simd_length(right) > 1e-3 { right = simd_normalize(right) }
        if simd_length(forward) > 1e-3 { forward = simd_normalize(forward) }
        let latest = snapshot.position
        trail = positions.map { entry in
            let d = entry.position - latest
            return CGPoint(x: CGFloat(simd_dot(d, right) * 1000), y: CGFloat(-simd_dot(d, forward) * 1000))
        }
    }

    private static func sampleDots(_ depth: FrameSnapshot.Depth) -> [OverlayDot] {
        let w = FrameSnapshot.depthWidth, h = FrameSnapshot.depthHeight
        var result: [OverlayDot] = []
        result.reserveCapacity((w / 8) * (h / 8))
        depth.meters.withUnsafeBytes { meters in
            depth.confidence.withUnsafeBytes { (conf: UnsafeRawBufferPointer) in
                for y in stride(from: 4, to: h, by: 8) {
                    for x in stride(from: 4, to: w, by: 8) {
                        let i = y * w + x
                        let d = meters.loadUnaligned(fromByteOffset: i * 4, as: Float32.self)
                        guard d.isFinite, d >= 0.2, d <= 5 else { continue }
                        result.append(OverlayDot(point: CGPoint(x: (CGFloat(x) + 0.5) / CGFloat(w),
                                                                y: (CGFloat(y) + 0.5) / CGFloat(h)),
                                                 depth: d, confidence: conf[i]))
                    }
                }
            }
        }
        return result
    }

    private static func colorize(_ depth: FrameSnapshot.Depth) -> CGImage? {
        let w = FrameSnapshot.depthWidth, h = FrameSnapshot.depthHeight
        var pixels = [UInt8](repeating: 0, count: w * h * 4)
        depth.meters.withUnsafeBytes { meters in
            depth.confidence.withUnsafeBytes { (conf: UnsafeRawBufferPointer) in
                for i in 0..<(w * h) {
                    let d = meters.loadUnaligned(fromByteOffset: i * 4, as: Float32.self)
                    guard d.isFinite, d >= 0.2, d <= 5, conf[i] >= 1 else { continue }
                    let c = Turbo.table[Turbo.index(d)]
                    let alpha: Float = conf[i] == 2 ? 0.62 : 0.35
                    // Premultiplied RGBA.
                    pixels[i * 4] = UInt8(c.x * alpha * 255)
                    pixels[i * 4 + 1] = UInt8(c.y * alpha * 255)
                    pixels[i * 4 + 2] = UInt8(c.z * alpha * 255)
                    pixels[i * 4 + 3] = UInt8(alpha * 255)
                }
            }
        }
        guard let provider = CGDataProvider(data: Data(pixels) as CFData) else { return nil }
        return CGImage(width: w, height: h, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: w * 4,
                       space: CGColorSpaceCreateDeviceRGB(),
                       bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
                       provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
    }
}

struct OverlayCanvas: View {
    let model: OverlayModel

    var body: some View {
        Canvas { context, size in
            let transform = model.displayTransform
            func view(_ p: CGPoint) -> CGPoint {
                let q = p.applying(transform)
                return CGPoint(x: q.x * size.width, y: q.y * size.height)
            }

            if model.showDepth, let image = model.depthImage {
                var layer = context
                layer.scaleBy(x: size.width, y: size.height)
                layer.concatenate(transform)
                layer.draw(Image(decorative: image, scale: 1), in: CGRect(x: 0, y: 0, width: 1, height: 1))
            }

            if model.showGrid {
                // Commitment tiling of the 256×192 analysis raster: 8×6 tiles of 32×32 px.
                var grid = Path()
                for column in 0...8 {
                    let x = CGFloat(column) / 8
                    grid.move(to: view(CGPoint(x: x, y: 0)))
                    grid.addLine(to: view(CGPoint(x: x, y: 1)))
                }
                for row in 0...6 {
                    let y = CGFloat(row) / 6
                    grid.move(to: view(CGPoint(x: 0, y: y)))
                    grid.addLine(to: view(CGPoint(x: 1, y: y)))
                }
                context.stroke(grid, with: .color(.white.opacity(0.28)), lineWidth: 0.7)
                // Central stratum (tiles 2...5 × 1...4) used by the central-planarity checks.
                var central = Path()
                central.move(to: view(CGPoint(x: 2.0 / 8, y: 1.0 / 6)))
                central.addLine(to: view(CGPoint(x: 6.0 / 8, y: 1.0 / 6)))
                central.addLine(to: view(CGPoint(x: 6.0 / 8, y: 5.0 / 6)))
                central.addLine(to: view(CGPoint(x: 2.0 / 8, y: 5.0 / 6)))
                central.closeSubpath()
                context.stroke(central, with: .color(.cyan.opacity(0.7)), lineWidth: 1.2)
            }

            let origin = view(model.principal)
            if model.showRays {
                for (index, dot) in model.dots.enumerated() where index % 3 == 0 {
                    var ray = Path()
                    ray.move(to: origin)
                    ray.addLine(to: view(dot.point))
                    context.stroke(ray, with: .color(Turbo.color(dot.depth).opacity(0.35)), lineWidth: 0.6)
                }
            }

            if model.showDots {
                for dot in model.dots {
                    let p = view(dot.point)
                    let radius: CGFloat = dot.confidence == 2 ? 2.6 : (dot.confidence == 1 ? 1.8 : 1.1)
                    let rect = CGRect(x: p.x - radius, y: p.y - radius, width: radius * 2, height: radius * 2)
                    context.fill(Path(ellipseIn: rect),
                                 with: .color(Turbo.color(dot.depth).opacity(dot.confidence == 0 ? 0.35 : 0.9)))
                }
            }

            if model.showLevel {
                // Horizon through the principal point, perpendicular to the projected world up.
                let up = model.upPixels
                let px = CGPoint(x: model.principal.x * CGFloat(FrameSnapshot.width),
                                 y: model.principal.y * CGFloat(FrameSnapshot.height))
                func normalized(_ p: CGPoint) -> CGPoint {
                    CGPoint(x: p.x / CGFloat(FrameSnapshot.width), y: p.y / CGFloat(FrameSnapshot.height))
                }
                let half: CGFloat = 520
                let a = view(normalized(CGPoint(x: px.x - up.dy * half, y: px.y + up.dx * half)))
                let b = view(normalized(CGPoint(x: px.x + up.dy * half, y: px.y - up.dx * half)))
                let tip = view(normalized(CGPoint(x: px.x + up.dx * 120, y: px.y + up.dy * 120)))
                let screenAngle = atan2(tip.x - origin.x, -(tip.y - origin.y)) * 180 / .pi
                let isLevel = abs(screenAngle) < 2
                var horizon = Path()
                horizon.move(to: a)
                horizon.addLine(to: b)
                context.stroke(horizon, with: .color((isLevel ? Color.green : Color.yellow).opacity(0.8)),
                               style: StrokeStyle(lineWidth: 1.4, dash: [8, 6]))
                var upTick = Path()
                upTick.move(to: origin)
                upTick.addLine(to: tip)
                context.stroke(upTick, with: .color(.white.opacity(0.7)), lineWidth: 1.2)
            }

            // Principal point crosshair.
            var cross = Path()
            cross.move(to: CGPoint(x: origin.x - 10, y: origin.y))
            cross.addLine(to: CGPoint(x: origin.x + 10, y: origin.y))
            cross.move(to: CGPoint(x: origin.x, y: origin.y - 10))
            cross.addLine(to: CGPoint(x: origin.x, y: origin.y + 10))
            context.stroke(cross, with: .color(.white.opacity(0.85)), lineWidth: 1)
        }
        // Rasterize offscreen (Metal) so redraws stay cheap on the main thread that also receives ARKit frames.
        .drawingGroup()
        .allowsHitTesting(false)
    }
}

/// Top-down path of the camera over the last 2 s in its own horizontal frame (1 grid cell = 10 mm).
struct MotionTrail: View {
    let model: OverlayModel

    var body: some View {
        Canvas { context, size in
            let center = CGPoint(x: size.width / 2, y: size.height / 2)
            let scale = size.width / 100   // ±50 mm across the box
            var grid = Path()
            for k in -5...5 {
                let offset = CGFloat(k) * 10 * scale
                grid.move(to: CGPoint(x: center.x + offset, y: 0))
                grid.addLine(to: CGPoint(x: center.x + offset, y: size.height))
                grid.move(to: CGPoint(x: 0, y: center.y + offset))
                grid.addLine(to: CGPoint(x: size.width, y: center.y + offset))
            }
            context.stroke(grid, with: .color(.white.opacity(0.12)), lineWidth: 0.5)
            let points = model.trail.map { CGPoint(x: center.x + $0.x * scale, y: center.y + $0.y * scale) }
            if points.count > 1 {
                var path = Path()
                path.addLines(points)
                context.stroke(path, with: .color(.cyan.opacity(0.9)), lineWidth: 1.5)
            }
            for (index, p) in points.enumerated() {
                let r: CGFloat = index == points.count - 1 ? 3.5 : 1.5
                context.fill(Path(ellipseIn: CGRect(x: p.x - r, y: p.y - r, width: 2 * r, height: 2 * r)),
                             with: .color(index == points.count - 1 ? .white : .cyan.opacity(0.7)))
            }
        }
        .frame(width: 104, height: 104)
        .background(.black.opacity(0.55), in: RoundedRectangle(cornerRadius: 8))
        .overlay(alignment: .bottomLeading) {
            Text("2 s · 10 mm").font(.system(size: 9).monospacedDigit()).foregroundStyle(.white.opacity(0.6)).padding(4)
        }
        .allowsHitTesting(false)
    }
}
