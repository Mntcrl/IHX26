import SwiftUI
import ARKit
import AVFoundation
import Photos
import CoreImage
import Combine
import ImageIO
import Metal
import QuartzCore

private enum CameraStyle {
    static let ink = Color(red: 0.035, green: 0.047, blue: 0.055)
    static let panel = Color(red: 0.075, green: 0.090, blue: 0.098)
    static let lime = Color(red: 0.78, green: 0.98, blue: 0.36)
    static let muted = Color.white.opacity(0.56)
}

nonisolated private struct Capture: Identifiable {
    let id: UUID
    let url: URL
    let depthURL: URL?
    let jsonURL: URL?
    let nativeFrameURL: URL?
    let date: Date

    var extraEvidence: [URL] {
        (["radio.json", "spatial.json", "imu.json", "noise.json", "noise.i16", "session.json", "anchors.json", "proof.json"] + PhysicsBurst.suffixes).compactMap { suffix in
            let file = evidenceURL(suffix)
            return FileManager.default.fileExists(atPath: file.path) ? file : nil
        }
    }

    func evidenceURL(_ suffix: String) -> URL {
        url.deletingPathExtension().appendingPathExtension(suffix)
    }
}

nonisolated private struct LiDARSegment: Sendable {
    let start: CGPoint
    let end: CGPoint
    let depth: CGFloat
}

nonisolated private struct ScanGeometry: Sendable {
    var vertices: [[Double]] = []
    var edges: [[Int]] = []
    var lines: [LiDARSegment] = []
    var points: [CGPoint] = []
}

nonisolated private struct CameraViewport {
    let size: CGSize
    let orientation: UIInterfaceOrientation

    static let corners = [CGPoint.zero, CGPoint(x: 1, y: 0), CGPoint(x: 1, y: 1), CGPoint(x: 0, y: 1)]

    var imageOrientation: CGImagePropertyOrientation {
        switch orientation {
        case .landscapeRight: .up
        case .landscapeLeft: .down
        case .portraitUpsideDown: .left
        default: .right
        }
    }

    var depthImageOrientation: UIImage.Orientation {
        switch imageOrientation {
        case .up: .up
        case .down: .down
        case .left: .left
        default: .right
        }
    }

    var label: String {
        switch orientation {
        case .landscapeLeft: "landscapeLeft"
        case .landscapeRight: "landscapeRight"
        case .portraitUpsideDown: "portraitUpsideDown"
        default: "portrait"
        }
    }

    func imageToView(_ frame: ARFrame) -> CGAffineTransform {
        frame.displayTransform(for: orientation, viewportSize: size)
    }

    static func ray(_ point: CGPoint, camera: ARCamera) -> SIMD3<Float> {
        let k = camera.intrinsics
        let size = camera.imageResolution
        let x = Float(point.x * size.width - 0.5) - k.columns.2.x
        let y = Float(point.y * size.height - 0.5) - k.columns.2.y
        return SIMD3(x / k.columns.0.x, -y / k.columns.1.y, -1)
    }

    func fieldOfView(_ frame: ARFrame) -> (horizontal: Double, vertical: Double) {
        let inverse = imageToView(frame).inverted()
        func angle(_ a: CGPoint, _ b: CGPoint) -> Double {
            let first = simd_normalize(Self.ray(a.applying(inverse), camera: frame.camera))
            let second = simd_normalize(Self.ray(b.applying(inverse), camera: frame.camera))
            return Double(acos(min(1, max(-1, simd_dot(first, second))))) * 180 / .pi
        }
        return (angle(CGPoint(x: 0, y: 0.5), CGPoint(x: 1, y: 0.5)),
                angle(CGPoint(x: 0.5, y: 0), CGPoint(x: 0.5, y: 1)))
    }
}

@MainActor
private final class CameraModel: ObservableObject {
    @Published var hasLiDAR = false
    @Published var trackingText = "Looking for depth"
    @Published var depthImage: UIImage?
    @Published var meshLines: [LiDARSegment] = []
    @Published var meshPoints: [CGPoint] = []
    @Published var fovLines: [LiDARSegment] = []
    @Published var horizontalFOV = 0.0
    @Published var verticalFOV = 0.0
    @Published var poseReadout = "X 0.00  Y 0.00  Z 0.00 m"
    @Published var ambientReadout = "LIGHT —"
    @Published var meshFaceCount = 0
    @Published var featurePointCount = 0
    @Published var captures: [Capture] = []
    @Published var toast: String?
    @Published var showDepth = false
    @Published var gridOn = true
    @Published var vectorsOn = true
    @Published var fovOn = true
    @Published var roomMeshOn = false
    @Published var roomMeshPath = Path()
    @Published var isCapturing = false
    weak var session: ARSession?
    private var lastDepthUpdate = Date.distantPast
    weak var cameraView: ARSCNView?
    private var lastViewport: CameraViewport?
    private var viewport: CameraViewport? {
        guard let view = cameraView, let scene = view.window?.windowScene,
              view.bounds.width > 0, view.bounds.height > 0 else { return nil }
        let orientation = scene.effectiveGeometry.interfaceOrientation
        guard orientation != .unknown else { return nil }
        return CameraViewport(size: view.bounds.size, orientation: orientation)
    }
    @Published var angleReadout = "YAW — · PITCH —"
    @Published var aimReadout = "DEPTH —"
    @Published var exposureReadout = "EXPOSURE —"
    private nonisolated static let ciContext = CIContext()

    private let captureSession = CaptureSessionManager()

    init() { loadCaptures() }

    func setCaptureInterfaceActive(_ active: Bool) { captureSession.setActive(active) }

    func configure(_ session: ARSession) {
        self.session = session
        guard ARWorldTrackingConfiguration.supportsFrameSemantics(.sceneDepth) else {
            trackingText = "LiDAR unavailable"
            return
        }
        hasLiDAR = true
        let configuration = ARWorldTrackingConfiguration()
        configuration.frameSemantics.insert(.sceneDepth)
        configuration.planeDetection = [.horizontal, .vertical]
        if ARWorldTrackingConfiguration.supportsSceneReconstruction(.meshWithClassification) {
            configuration.sceneReconstruction = .meshWithClassification
        } else if ARWorldTrackingConfiguration.supportsSceneReconstruction(.mesh) {
            configuration.sceneReconstruction = .mesh
        }
        session.run(configuration, options: [.resetTracking, .removeExistingAnchors])
    }

    func received(_ frame: ARFrame) {
        captureSession.record(frame)
        guard let viewport else { return }
        let rotated = lastViewport?.orientation != viewport.orientation || lastViewport?.size != viewport.size
        let now = Date()
        guard rotated || now.timeIntervalSince(lastDepthUpdate) > 0.08 else { return }
        lastViewport = viewport
        lastDepthUpdate = now

        let camera = frame.camera
        let fov = viewport.fieldOfView(frame)
        horizontalFOV = fov.horizontal
        verticalFOV = fov.vertical
        let position = camera.transform.columns.3
        poseReadout = String(format: "X %+.2f  Y %+.2f  Z %+.2f m", position.x, position.y, position.z)
        let forward = -camera.transform.columns.2
        let yaw = atan2(forward.x, -forward.z) * 180 / .pi
        let pitch = atan2(forward.y, hypot(forward.x, forward.z)) * 180 / .pi
        angleReadout = String(format: "YAW %+.1f° · PITCH %+.1f°", yaw, pitch)
        ambientReadout = frame.lightEstimate.map {
            String(format: "LIGHT %.0f · %.0f K", $0.ambientIntensity, $0.ambientColorTemperature)
        } ?? "LIGHT —"
        exposureReadout = String(format: "%.1f ms · %+.1f EV", camera.exposureDuration * 1000, camera.exposureOffset)
        let meshAnchors = frame.anchors.compactMap { $0 as? ARMeshAnchor }
        meshFaceCount = meshAnchors.reduce(0) { $0 + $1.geometry.faces.count }
        if roomMeshOn {
            roomMeshPath = Self.projectRoomMesh(meshAnchors, faceCount: meshFaceCount, frame: frame, viewport: viewport)
        }
        featurePointCount = frame.rawFeaturePoints?.points.count ?? 0
        fovLines = Self.makeFOVLines(frame, viewport: viewport)

        if let depth = frame.sceneDepth {
            trackingText = "Depth active"
            let target = CGPoint(x: 0.5, y: 0.5).applying(viewport.imageToView(frame).inverted())
            aimReadout = Self.depthAt(target, in: depth.depthMap).map {
                String(format: "CENTER %.2f m", $0)
            } ?? "CENTER —"
            depthImage = showDepth ? Self.makeDepthImage(depth.depthMap, orientation: viewport.depthImageOrientation) : nil
            if vectorsOn {
                let geometry = Self.makeScanGeometry(frame, viewport: viewport)
                meshLines = geometry.lines
                meshPoints = geometry.points
            }
        } else {
            trackingText = camera.trackingState == .normal ? "Move to scan" : "Finding surfaces"
            aimReadout = "CENTER —"
            depthImage = nil
            meshLines = []
            meshPoints = []
        }
    }

    private static func depthAt(_ point: CGPoint, in buffer: CVPixelBuffer) -> Float? {
        guard point.x >= 0, point.x < 1, point.y >= 0, point.y < 1,
              CVPixelBufferLockBaseAddress(buffer, .readOnly) == kCVReturnSuccess else { return nil }
        defer { CVPixelBufferUnlockBaseAddress(buffer, .readOnly) }
        let x = Int(point.x * CGFloat(CVPixelBufferGetWidth(buffer)))
        let y = Int(point.y * CGFloat(CVPixelBufferGetHeight(buffer)))
        guard let base = CVPixelBufferGetBaseAddress(buffer) else { return nil }
        let row = base.advanced(by: y * CVPixelBufferGetBytesPerRow(buffer)).assumingMemoryBound(to: Float.self)
        let value = row[x]
        return value.isFinite && value > 0 ? value : nil
    }

    func capture() {
        guard !isCapturing else { return }
        guard let session, let frame = session.currentFrame, let viewport else {
            announce("Camera is not ready")
            return
        }
        isCapturing = true
        let window = captureSession.begin(frame)
        let format = session.configuration.map {
            let video = $0.videoFormat
            return CameraEvidence.VideoFormat(
                deviceType: video.captureDeviceType.rawValue,
                cameraPosition: video.captureDevicePosition.rawValue,
                framesPerSecond: video.framesPerSecond,
                hdrSupported: video.isVideoHDRSupported
            )
        }
        // Shutter-only work starts immediately, alongside the short collection tail.
        let photoTask = Task.detached(priority: .userInitiated) { try Self.preparePhoto(frame, viewport: viewport) }
        let cameraTask = Task.detached(priority: .userInitiated) { () throws -> PreparedCamera? in
            guard let native = try CameraEvidence.read(frame.capturedImage) else { return nil }
            return PreparedCamera(native: native, residual: try CameraNoiseAnalysis.prepareSpatial(native))
        }
        let depthTask = Task.detached(priority: .userInitiated) { () throws -> PreparedDepth in
            let depthData: Data?
            if let sceneDepth = frame.sceneDepth {
                guard let packed = Self.makeDepthData(sceneDepth.depthMap) else { throw CocoaError(.fileReadCorruptFile) }
                depthData = packed
            } else {
                depthData = nil
            }
            return PreparedDepth(samples: depthData, geometry: Self.makeScanGeometry(frame, viewport: nil))
        }
        Task {
            defer {
                photoTask.cancel()
                cameraTask.cancel()
                depthTask.cancel()
            }
            let ambient = await captureSession.finish(window)
            do {
                let photo = try await photoTask.value
                let camera = try await cameraTask.value
                let depth = try await depthTask.value
                let (capture, root) = try await Task.detached(priority: .userInitiated) {
                    try Self.saveCapture(frame, viewport: viewport, format: format, ambient: ambient,
                                         photo: photo, camera: camera, depth: depth)
                }.value
                captures.insert(capture, at: 0)
                isCapturing = false
                announce("Saved to Scans")
                async let receipt = captureSession.submit(root: root, window: window)
                await Self.saveToPhotos(capture.url)
                if let result = await receipt, result.xrplAnchoringVerified { announce("XRPL anchor verified") }
            } catch {
                isCapturing = false
                announce("Could not save scan")
            }
        }
    }

    private nonisolated struct PreparedPhoto: Sendable {
        let jpeg: Data
        let size: CGSize
    }

    private nonisolated struct PreparedCamera: Sendable {
        let native: CameraEvidence.Frame
        let residual: CameraNoiseAnalysis.Spatial
    }

    private nonisolated struct PreparedDepth: Sendable {
        let samples: Data?
        let geometry: ScanGeometry
    }

    private nonisolated static func preparePhoto(_ frame: ARFrame, viewport: CameraViewport) throws -> PreparedPhoto {
        let image = CIImage(cvPixelBuffer: frame.capturedImage).oriented(viewport.imageOrientation)
        let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
        let compression = CIImageRepresentationOption(rawValue: kCGImageDestinationLossyCompressionQuality as String)
        guard let jpeg = ciContext.jpegRepresentation(of: image, colorSpace: colorSpace,
                                                       options: [compression: 0.94]) else {
            throw CocoaError(.fileWriteUnknown)
        }
        return PreparedPhoto(jpeg: jpeg, size: image.extent.size)
    }

    private nonisolated static func saveCapture(_ frame: ARFrame, viewport: CameraViewport, format: CameraEvidence.VideoFormat?,
                                                ambient: CompletedCaptureWindow, photo: PreparedPhoto,
                                                camera: PreparedCamera?, depth: PreparedDepth) throws -> (Capture, String) {
        let date = ambient.window.date
        let jpeg = photo.jpeg
        let id = UUID()
        let name = "scan-\(id.uuidString)"
        let documents = try FileManager.default.url(for: .documentDirectory, in: .userDomainMask,
                                                    appropriateFor: nil, create: true)
        let url = documents.appendingPathComponent(name).appendingPathExtension("jpg")
        let jsonURL = url.deletingPathExtension().appendingPathExtension("json")
        let rawDepthURL = url.deletingPathExtension().appendingPathExtension("fdepth")
        let nativeURL = url.deletingPathExtension().appendingPathExtension("yuv")
        let capture = Capture(id: id, url: url, depthURL: frame.sceneDepth == nil ? nil : rawDepthURL,
                              jsonURL: jsonURL, nativeFrameURL: nil, date: date)
        let radioURL = capture.evidenceURL("radio.json")
        let spatialURL = capture.evidenceURL("spatial.json")
        let sessionURL = capture.evidenceURL("session.json")
        let anchorsURL = capture.evidenceURL("anchors.json")
        let proofURL = capture.evidenceURL("proof.json")
        let imuURL = capture.evidenceURL("imu.json")
        let noiseJSONURL = capture.evidenceURL("noise.json")
        let noiseSamplesURL = capture.evidenceURL("noise.i16")
        var photoAttachments: [String: String] = [:]
        do {
            let nativeFrame = camera?.native
            if nativeFrame != nil {
                photoAttachments = try PhysicsBurst.write(shutter: frame, references: ambient.cameraReferences,
                                                          center: ambient.window.center, sourcePhoto: url,
                                                          depthData: makeDepthData)
            }
            var evidence: [String: Any] = [
                "arkitGrainIntensity": frame.cameraGrainIntensity,
                "arkitGrainMeaning": "ARKit rendering parameter, separate from the measured luma residual",
                "digestAlgorithm": "SHA-256",
                "digestScope": "Integrity checks for exported files; unsigned"
            ]
            if let format { evidence["configuredVideoFormat"] = format.metadata }
            var hashes = [url.lastPathComponent: CameraEvidence.sha256(jpeg)]
            if let camera, let nativeFrame {
                try nativeFrame.samples.write(to: nativeURL, options: .atomic)
                var metadata = nativeFrame.metadata
                metadata["file"] = nativeURL.lastPathComponent
                evidence["nativeFrame"] = metadata
                hashes[nativeURL.lastPathComponent] = CameraEvidence.sha256(nativeFrame.samples)
                let noiseJSON = try CameraNoiseAnalysis.metadata(camera.residual, native: nativeFrame, frame: frame,
                                                                 references: ambient.cameraReferences, fileURL: noiseSamplesURL)
                try camera.residual.samples.write(to: noiseSamplesURL, options: .atomic)
                try noiseJSON.write(to: noiseJSONURL, options: .atomic)
                photoAttachments[noiseJSONURL.lastPathComponent] = CameraEvidence.sha256(noiseJSON)
                photoAttachments[noiseSamplesURL.lastPathComponent] = CameraEvidence.sha256(camera.residual.samples)
                evidence["fullResolutionNoiseAnalysisFile"] = noiseJSONURL.lastPathComponent
            } else {
                evidence["nativeFrame"] = ["status": "unsupported pixel layout",
                                           "pixelFormatFourCC": CameraEvidence.fourCC(CVPixelBufferGetPixelFormatType(frame.capturedImage))]
            }
            var depthURL: URL?
            if let packedDepth = depth.samples {
                try packedDepth.write(to: rawDepthURL, options: .atomic)
                depthURL = rawDepthURL
                hashes[rawDepthURL.lastPathComponent] = CameraEvidence.sha256(packedDepth)
            }
            evidence["fileDigests"] = hashes.merging(photoAttachments) { _, new in new }
            try writeManifest(frame: frame, captureID: id.uuidString, date: date, imageSize: photo.size,
                              depthURL: depthURL, geometry: depth.geometry, viewport: viewport,
                              cameraEvidence: evidence, to: jsonURL)
            let radio = try EvidenceCommitmentBuilder.canonical(ambient.radio)
            try radio.write(to: radioURL, options: .atomic)
            let spatial = SpatialEvidenceCollector.collect(frame: frame, poses: ambient.poses, center: ambient.window.center,
                                                           clock: ambient.window.clock)
            let spatialBytes = try ScanExport.encodeJSON(spatial)
            try spatialBytes.write(to: spatialURL, options: .atomic)
            let imu = try EvidenceCommitmentBuilder.canonical(ambient.inertial)
            try imu.write(to: imuURL, options: .atomic)
            let photoHash = hashes[url.lastPathComponent] ?? CameraEvidence.sha256(jpeg)
            photoAttachments[url.lastPathComponent] = photoHash
            var categories = [
                "photo": photoAttachments,
                "lidar": [jsonURL.lastPathComponent: try EvidenceCommitmentBuilder.fileHash(jsonURL),
                          spatialURL.lastPathComponent: CameraEvidence.sha256(spatialBytes),
                          imuURL.lastPathComponent: CameraEvidence.sha256(imu)],
                "radio": [radioURL.lastPathComponent: CameraEvidence.sha256(radio)]
            ]
            if let depthURL { categories["lidar"]?[depthURL.lastPathComponent] = hashes[depthURL.lastPathComponent] }
            if let nativeHash = hashes[nativeURL.lastPathComponent] { categories["photo"]?[nativeURL.lastPathComponent] = nativeHash }
            if !ambient.trustedAnchors.isEmpty {
                let anchors = try EvidenceCommitmentBuilder.canonical(ambient.trustedAnchors)
                try anchors.write(to: anchorsURL, options: .atomic)
                categories["trustedAnchors"] = [anchorsURL.lastPathComponent: CameraEvidence.sha256(anchors)]
            }
            let metadata = AmbientCaptureEvidence(
                protocolVersion: 1, sessionID: ambient.window.sessionID, serverChallenge: ambient.window.challenge,
                claimedCaptureTimestamp: Int64((date.timeIntervalSince1970 * 1000).rounded()),
                observationWindow: Int(((ambient.window.preSeconds + ambient.window.postSeconds) * 1000).rounded()),
                observationPreMilliseconds: Int((ambient.window.preSeconds * 1000).rounded()),
                observationPostMilliseconds: Int((ambient.window.postSeconds * 1000).rounded()),
                shutterToCommitPreparationMilliseconds: Int(((CACurrentMediaTime() - ambient.window.shutterUptime) * 1000).rounded()),
                photographHash: photoHash,
                lidarEvidenceHash: EvidenceCommitmentBuilder.hex(try EvidenceCommitmentBuilder.payloadDigest(categories["lidar"] ?? [:])),
                radioFingerprintHash: CameraEvidence.sha256(radio),
                sensorAvailability: ["photo": true, "sceneDepth": depthURL != nil, "radio": ambient.radio.scanDurationMilliseconds > 0,
                                     "cameraMotion": !ambient.poses.isEmpty, "trustedAnchors": !ambient.trustedAnchors.isEmpty,
                                     "accelerometer": ambient.inertial.accelerometer.sampleCount > 0,
                                     "gyroscope": ambient.inertial.gyroscope.sampleCount > 0]
            )
            let metadataBytes = try EvidenceCommitmentBuilder.canonical(metadata)
            try metadataBytes.write(to: sessionURL, options: .atomic)
            categories["metadata"] = [sessionURL.lastPathComponent: CameraEvidence.sha256(metadataBytes)]
            let proof = try EvidenceCommitmentBuilder.build(categories: categories)
            try EvidenceCommitmentBuilder.canonical(proof).write(to: proofURL, options: .atomic)
            // The library discovers captures by JPEG, so publish it after all sidecars.
            try jpeg.write(to: url, options: .atomic)
            return (Capture(id: id, url: url, depthURL: depthURL, jsonURL: jsonURL,
                            nativeFrameURL: nativeFrame == nil ? nil : nativeURL, date: date), proof.root)
        } catch {
            for file in [url, jsonURL, rawDepthURL, nativeURL, radioURL, spatialURL, imuURL, noiseJSONURL, noiseSamplesURL,
                         sessionURL, anchorsURL, proofURL] + PhysicsBurst.suffixes.map(capture.evidenceURL) {
                try? FileManager.default.removeItem(at: file)
            }
            throw error
        }
    }

    private nonisolated static func saveToPhotos(_ url: URL) async {
        let status = await PHPhotoLibrary.requestAuthorization(for: .addOnly)
        guard status == .authorized || status == .limited else { return }
        let changes: @Sendable () -> Void = {
            PHAssetChangeRequest.creationRequestForAssetFromImage(atFileURL: url)
        }
        try? await PHPhotoLibrary.shared().performChanges(changes)
    }

    func announce(_ message: String) {
        toast = message
        Task { @MainActor in
            try? await Task.sleep(for: .seconds(2))
            if toast == message { toast = nil }
        }
    }

    private func loadCaptures() {
        let folder = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        let urls = (try? FileManager.default.contentsOfDirectory(at: folder, includingPropertiesForKeys: [.creationDateKey])) ?? []
        captures = urls.filter { $0.lastPathComponent.hasPrefix("scan-") && $0.pathExtension == "jpg" }
            .sorted { $0.lastPathComponent > $1.lastPathComponent }
            .map {
                let depth = $0.deletingPathExtension().appendingPathExtension("fdepth")
                let json = $0.deletingPathExtension().appendingPathExtension("json")
                let native = $0.deletingPathExtension().appendingPathExtension("yuv")
                return Capture(id: UUID(), url: $0, depthURL: FileManager.default.fileExists(atPath: depth.path) ? depth : nil,
                               jsonURL: FileManager.default.fileExists(atPath: json.path) ? json : nil,
                               nativeFrameURL: FileManager.default.fileExists(atPath: native.path) ? native : nil,
                               date: (try? $0.resourceValues(forKeys: [.creationDateKey]).creationDate) ?? Date())
            }
    }

    private nonisolated static func makeScanGeometry(_ frame: ARFrame, viewport: CameraViewport?) -> ScanGeometry {
        guard let buffer = frame.sceneDepth?.depthMap,
              CVPixelBufferLockBaseAddress(buffer, .readOnly) == kCVReturnSuccess else { return ScanGeometry() }
        defer { CVPixelBufferUnlockBaseAddress(buffer, .readOnly) }
        let width = CVPixelBufferGetWidth(buffer), height = CVPixelBufferGetHeight(buffer)
        let rowBytes = CVPixelBufferGetBytesPerRow(buffer)
        guard let base = CVPixelBufferGetBaseAddress(buffer), width > 0, height > 0 else { return ScanGeometry() }

        let step = 12
        let xs = Array(stride(from: 0, to: width, by: step))
        let ys = Array(stride(from: 0, to: height, by: step))
        let display = viewport?.imageToView(frame)
        let transform = frame.camera.transform
        var result = ScanGeometry()
        var gridIndices = Array(repeating: Optional<Int>.none, count: xs.count * ys.count)
        var depths = Array(repeating: Optional<Float>.none, count: xs.count * ys.count)
        var projected = Array(repeating: Optional<CGPoint>.none, count: xs.count * ys.count)

        for (gy, y) in ys.enumerated() {
            let row = UnsafeRawPointer(base.advanced(by: y * rowBytes)).assumingMemoryBound(to: Float32.self)
            for (gx, x) in xs.enumerated() {
                let depth = row[x]
                guard depth.isFinite, depth > 0.15, depth < 8 else { continue }
                let sample = CGPoint(x: (CGFloat(x) + 0.5) / CGFloat(width),
                                     y: (CGFloat(y) + 0.5) / CGFloat(height))
                let ray = CameraViewport.ray(sample, camera: frame.camera)
                let cameraPoint = SIMD4<Float>(ray.x * depth, ray.y * depth, -depth, 1)
                let world4 = transform * cameraPoint
                let world = SIMD3<Float>(world4.x, world4.y, world4.z)
                let index = result.vertices.count
                result.vertices.append([Double(world.x), Double(world.y), Double(world.z)])
                let gridIndex = gy * xs.count + gx
                gridIndices[gridIndex] = index
                depths[gridIndex] = depth
                if let viewport, let display {
                    let point = sample.applying(display)
                    let projectedPoint = CGPoint(x: point.x * viewport.size.width, y: point.y * viewport.size.height)
                    projected[gridIndex] = projectedPoint
                    if (0...1).contains(point.x), (0...1).contains(point.y) {
                        result.points.append(projectedPoint)
                    }
                }
            }
        }

        for gy in 0..<ys.count {
            for gx in 0..<xs.count {
                let index = gy * xs.count + gx
                guard let vertex = gridIndices[index], let depth = depths[index] else { continue }
                for (nx, ny) in [(gx + 1, gy), (gx, gy + 1)] where nx < xs.count && ny < ys.count {
                    let neighborIndex = ny * xs.count + nx
                    guard let neighbor = gridIndices[neighborIndex], let neighborDepth = depths[neighborIndex] else { continue }
                    let difference = abs(depth - neighborDepth)
                    guard difference < max(0.18, min(depth, neighborDepth) * 0.12) else { continue }
                    result.edges.append([vertex, neighbor])
                    if let a = projected[index], let b = projected[neighborIndex] {
                        result.lines.append(LiDARSegment(start: a, end: b, depth: CGFloat((depth + neighborDepth) * 0.5)))
                    }
                }
            }
        }
        return result
    }

    private static func projectRoomMesh(_ anchors: [ARMeshAnchor], faceCount: Int,
                                        frame: ARFrame, viewport: CameraViewport) -> Path {
        var path = Path()
        // Sample only the onscreen preview. The export uses all mesh faces.
        let strideLength = max(1, (faceCount + 799) / 800)
        let worldToCamera = frame.camera.transform.inverse
        var skippedFaces = 0
        for anchor in anchors {
            let vertices = anchor.geometry.vertices
            let faces = anchor.geometry.faces
            guard faces.indexCountPerPrimitive == 3 else { continue }
            let localToCamera = worldToCamera * anchor.transform
            let first = (strideLength - skippedFaces % strideLength) % strideLength
            skippedFaces += faces.count
            for face in stride(from: first, to: faces.count, by: strideLength) {
                var points: [CGPoint] = []
                for corner in 0..<3 {
                    let index = meshIndex(faces, at: face * 3 + corner)
                    guard index < vertices.count else { break }
                    let vertex = meshVector(vertices, at: index)
                    let local = SIMD4<Float>(vertex.x, vertex.y, vertex.z, 1)
                    guard (localToCamera * local).z < -0.05 else { break }
                    let world = anchor.transform * local
                    let point = frame.camera.projectPoint(SIMD3(world.x, world.y, world.z),
                                                          orientation: viewport.orientation, viewportSize: viewport.size)
                    guard point.x.isFinite, point.y.isFinite else { break }
                    points.append(point)
                }
                guard points.count == 3 else { continue }
                path.move(to: points[0])
                path.addLine(to: points[1])
                path.addLine(to: points[2])
                path.closeSubpath()
            }
        }
        return path
    }

    private static func makeFOVLines(_ frame: ARFrame, viewport: CameraViewport) -> [LiDARSegment] {
        let intrinsics = frame.camera.intrinsics
        let resolution = frame.camera.imageResolution
        let principal = CGPoint(x: (CGFloat(intrinsics.columns.2.x) + 0.5) / resolution.width,
                                y: (CGFloat(intrinsics.columns.2.y) + 0.5) / resolution.height)
            .applying(viewport.imageToView(frame))
        func screen(_ point: CGPoint) -> CGPoint {
            CGPoint(x: point.x * viewport.size.width, y: point.y * viewport.size.height)
        }
        return CameraViewport.corners.map { LiDARSegment(start: screen(principal), end: screen($0), depth: 0) }
    }

    private nonisolated static func writeManifest(frame: ARFrame, captureID: String, date: Date, imageSize: CGSize,
                                                 depthURL: URL?, geometry: ScanGeometry, viewport: CameraViewport,
                                                 cameraEvidence: [String: Any], to url: URL) throws {
        let transform = frame.camera.transform
        let camera = frame.camera
        let resolution = camera.imageResolution
        let intrinsics = camera.intrinsics
        let viewMatrix = camera.viewMatrix(for: viewport.orientation)
        let projectionMatrix = camera.projectionMatrix(for: viewport.orientation, viewportSize: viewport.size,
                                                       zNear: 0.01, zFar: 1000)
        let display = viewport.imageToView(frame)
        let fov = viewport.fieldOfView(frame)
        let imageWidth = Int(imageSize.width)
        let imageHeight = Int(imageSize.height)
        var depthDescription: [String: Any]?
        var depthResolution: [String: Int]?
        if let depth = frame.sceneDepth, let depthURL {
            depthResolution = ["width": CVPixelBufferGetWidth(depth.depthMap), "height": CVPixelBufferGetHeight(depth.depthMap)]
            depthDescription = ["file": depthURL.lastPathComponent,
                                "format": "FLDP header, UInt32LE width and height, then tightly packed IEEE754 Float32LE meters",
                                "units": "meters", "coordinates": "native sensor", "measurement": "distance along the optical axis"]
            if let confidence = depth.confidenceMap, let bytes = Self.confidenceBytes(confidence) {
                depthDescription?["confidence"] = ["width": CVPixelBufferGetWidth(confidence),
                                                   "height": CVPixelBufferGetHeight(confidence),
                                                   "format": "row-major UInt8, base64; 0 low, 1 medium, 2 high",
                                                   "data": bytes.base64EncodedString()]
            }
        }
        let position = transform.columns.3
        let featurePoints = frame.rawFeaturePoints?.points.map { [Double($0.x), Double($0.y), Double($0.z)] } ?? []
        let featureIDs = frame.rawFeaturePoints?.identifiers.map(String.init) ?? []
        let meshAnchors = frame.anchors.compactMap { $0 as? ARMeshAnchor }
        let planes = frame.anchors.compactMap { $0 as? ARPlaneAnchor }.map(planeAnchorManifest)
        let names = ["topLeft", "topRight", "bottomRight", "bottomLeft"]
        func cornerRays(sensorToOutput: CGAffineTransform) -> [[String: Any]] {
            let inverse = sensorToOutput.inverted()
            return zip(names, CameraViewport.corners).map { name, corner in
                let sensorPoint = corner.applying(inverse)
                let cameraDirection = simd_normalize(CameraViewport.ray(sensorPoint, camera: camera))
                let worldDirection4 = transform * SIMD4<Float>(cameraDirection.x, cameraDirection.y, cameraDirection.z, 0)
                let worldDirection = simd_normalize(SIMD3<Float>(worldDirection4.x, worldDirection4.y, worldDirection4.z))
                return ["corner": name, "sensorNormalizedXY": [sensorPoint.x, sensorPoint.y],
                        "originWorldMeters": [position.x, position.y, position.z],
                        "directionCamera": vectorArray(cameraDirection), "directionWorld": vectorArray(worldDirection)]
            }
        }
        let photoViewport = CameraViewport(size: CGSize(width: imageWidth, height: imageHeight),
                                           orientation: viewport.orientation)
        let photoFOV = photoViewport.fieldOfView(frame)
        let document: [String: Any] = [
            "schema": "com.fieldnote.spatial-scan",
            "schemaVersion": 5,
            "captureId": captureID,
            "capturedAt": ISO8601DateFormatter().string(from: date),
            "arFrameTimestampSeconds": frame.timestamp,
            "units": "meters",
            "coordinateFrame": "ARKit world coordinates at capture time",
            "tracking": ["cameraState": String(describing: frame.camera.trackingState),
                         "worldMappingStatus": String(describing: frame.worldMappingStatus)],
            "image": ["file": "scan-\(captureID).jpg", "width": imageWidth, "height": imageHeight,
                      "rotationFromSensorEXIF": viewport.imageOrientation.rawValue],
            "cameraEvidence": cameraEvidence,
            "depth": depthDescription as Any? ?? NSNull(),
            "depthResolution": depthResolution as Any? ?? NSNull(),
            "camera": [
                "intrinsics": [[Double(intrinsics.columns.0.x), 0, Double(intrinsics.columns.2.x)],
                               [0, Double(intrinsics.columns.1.y), Double(intrinsics.columns.2.y)], [0, 0, 1]],
                "imageResolution": [Int(resolution.width), Int(resolution.height)],
                "calibrationCoordinates": "native sensor, origin at center of top-left pixel, x right, y down",
                "rayCoordinates": "camera space x right, y up, forward -z",
                "horizontalFOVDegrees": fov.horizontal,
                "verticalFOVDegrees": fov.vertical,
                "exposureDurationSeconds": camera.exposureDuration,
                "exposureOffsetEV": Double(camera.exposureOffset),
                "grainIntensity": Double(frame.cameraGrainIntensity),
                "sourceEXIF": frame.exifData,
                "worldPositionMeters": [Double(position.x), Double(position.y), Double(position.z)],
                "worldTransformRowMajor": matrixRows(transform),
                "viewMatrixRowMajor": matrixRows(viewMatrix),
                "projectionMatrixRowMajor": matrixRows(projectionMatrix),
                "viewfinder": ["orientation": viewport.label, "sizePoints": [viewport.size.width, viewport.size.height],
                               "clipRangeMeters": [0.01, 1000],
                               "sensorToViewNormalizedRowMajor": [[display.a, display.c, display.tx],
                                                                  [display.b, display.d, display.ty], [0, 0, 1]]],
                "fovCornerRays": cornerRays(sensorToOutput: display),
                "photoFOV": ["horizontalDegrees": photoFOV.horizontal, "verticalDegrees": photoFOV.vertical,
                             "cornerRays": cornerRays(sensorToOutput: photoViewport.imageToView(frame))]
            ],
            "ambientLight": frame.lightEstimate.map { ["intensityARKit": Double($0.ambientIntensity),
                                                        "colorTemperatureKelvin": Double($0.ambientColorTemperature)] as [String: Any] } ?? NSNull(),
            "featureCloud": ["count": featurePoints.count, "pointsWorldMeters": featurePoints, "identifiers": featureIDs],
            "planeAnchors": planes,
            "pointCloud": ["vertices": geometry.vertices, "edges": geometry.edges,
                           "vertexCount": geometry.vertices.count, "edgeCount": geometry.edges.count,
                           "positionFormat": "[x, y, z] Float64 meters in ARKit world coordinates"]
        ]
        try ScanExport.writeManifest(document, meshCount: meshAnchors.count,
                                     meshAt: { meshAnchorManifest(meshAnchors[$0]) }, to: url)
    }

    private nonisolated static func matrixRows(_ matrix: simd_float4x4) -> [[Double]] {
        [
            [Double(matrix.columns.0.x), Double(matrix.columns.1.x), Double(matrix.columns.2.x), Double(matrix.columns.3.x)],
            [Double(matrix.columns.0.y), Double(matrix.columns.1.y), Double(matrix.columns.2.y), Double(matrix.columns.3.y)],
            [Double(matrix.columns.0.z), Double(matrix.columns.1.z), Double(matrix.columns.2.z), Double(matrix.columns.3.z)],
            [Double(matrix.columns.0.w), Double(matrix.columns.1.w), Double(matrix.columns.2.w), Double(matrix.columns.3.w)]
        ]
    }

    private nonisolated static func meshAnchorManifest(_ anchor: ARMeshAnchor) -> [String: Any] {
        let geometry = anchor.geometry
        let vertexSource = geometry.vertices
        let normalSource = geometry.normals
        let vertices = (0..<vertexSource.count).map { vectorArray(meshVector(vertexSource, at: $0)) }
        let normals = (0..<normalSource.count).map { vectorArray(meshVector(normalSource, at: $0)) }
        let element = geometry.faces
        let faces = (0..<element.count).map { face in
            (0..<element.indexCountPerPrimitive).map { meshIndex(element, at: face * element.indexCountPerPrimitive + $0) }
        }
        let classifications: [Int]?
        if let source = geometry.classification {
            classifications = (0..<min(source.count, geometry.faces.count)).map { index in
                Int(source.buffer.contents().advanced(by: source.offset + index * source.stride).load(as: UInt8.self))
            }
        } else { classifications = nil }
        return ["identifier": anchor.identifier.uuidString,
                "transformRowMajor": matrixRows(anchor.transform),
                "verticesLocalMeters": vertices,
                "normalsLocal": normals,
                "faces": faces,
                "faceClassifications": classifications as Any? ?? NSNull(),
                "classificationCodes": ["0": "none", "1": "wall", "2": "floor", "3": "ceiling", "4": "table", "5": "seat", "6": "window", "7": "door"]]
    }

    private nonisolated static func vectorArray(_ value: SIMD3<Float>) -> [Float] {
        [value.x, value.y, value.z]
    }

    private nonisolated static func meshVector(_ source: ARGeometrySource, at index: Int) -> SIMD3<Float> {
        let pointer = source.buffer.contents().advanced(by: source.offset + index * source.stride)
        return SIMD3(pointer.loadUnaligned(as: Float.self),
                     pointer.loadUnaligned(fromByteOffset: 4, as: Float.self),
                     pointer.loadUnaligned(fromByteOffset: 8, as: Float.self))
    }

    private nonisolated static func meshIndex(_ faces: ARGeometryElement, at index: Int) -> Int {
        let pointer = faces.buffer.contents().advanced(by: index * faces.bytesPerIndex)
        if faces.bytesPerIndex == 2 { return Int(pointer.loadUnaligned(as: UInt16.self)) }
        return Int(pointer.loadUnaligned(as: UInt32.self))
    }

    private nonisolated static func confidenceBytes(_ buffer: CVPixelBuffer) -> Data? {
        guard CVPixelBufferGetPixelFormatType(buffer) == kCVPixelFormatType_OneComponent8,
              CVPixelBufferLockBaseAddress(buffer, .readOnly) == kCVReturnSuccess else { return nil }
        defer { CVPixelBufferUnlockBaseAddress(buffer, .readOnly) }
        guard let base = CVPixelBufferGetBaseAddress(buffer) else { return nil }
        let width = CVPixelBufferGetWidth(buffer)
        let rowBytes = CVPixelBufferGetBytesPerRow(buffer)
        var data = Data()
        for row in 0..<CVPixelBufferGetHeight(buffer) {
            data.append(base.advanced(by: row * rowBytes).assumingMemoryBound(to: UInt8.self), count: width)
        }
        return data
    }

    private nonisolated static func planeAnchorManifest(_ anchor: ARPlaneAnchor) -> [String: Any] {
        let boundary = anchor.geometry.boundaryVertices.map { [Double($0.x), Double($0.y), Double($0.z)] }
        return ["identifier": anchor.identifier.uuidString,
                "alignment": String(describing: anchor.alignment),
                "classification": String(describing: anchor.classification),
                "centerLocalMeters": [Double(anchor.center.x), Double(anchor.center.y), Double(anchor.center.z)],
                "extentWidthHeightMeters": [Double(anchor.planeExtent.width), Double(anchor.planeExtent.height)],
                "extentRotationRadians": Double(anchor.planeExtent.rotationOnYAxis),
                "boundaryVerticesLocalMeters": boundary,
                "transformRowMajor": matrixRows(anchor.transform)]
    }

    // Fieldnote depth files contain the ASCII magic "FLDP", width and height as
    // little endian UInt32 values, then row-packed Float32 distances in meters.
    private nonisolated static func makeDepthData(_ buffer: CVPixelBuffer) -> Data? {
        guard CVPixelBufferLockBaseAddress(buffer, .readOnly) == kCVReturnSuccess else { return nil }
        defer { CVPixelBufferUnlockBaseAddress(buffer, .readOnly) }
        let width = CVPixelBufferGetWidth(buffer), height = CVPixelBufferGetHeight(buffer)
        let rowBytes = CVPixelBufferGetBytesPerRow(buffer)
        guard let base = CVPixelBufferGetBaseAddress(buffer), width > 0, height > 0 else { return nil }
        var data = Data("FLDP".utf8)
        var w = UInt32(width).littleEndian, h = UInt32(height).littleEndian
        withUnsafeBytes(of: &w) { data.append(contentsOf: $0) }
        withUnsafeBytes(of: &h) { data.append(contentsOf: $0) }
        for y in 0..<height {
            data.append(base.advanced(by: y * rowBytes).assumingMemoryBound(to: UInt8.self), count: width * MemoryLayout<Float32>.size)
        }
        return data
    }

    nonisolated private static func makeDepthImage(_ buffer: CVPixelBuffer, orientation: UIImage.Orientation) -> UIImage? {
        guard CVPixelBufferLockBaseAddress(buffer, .readOnly) == kCVReturnSuccess else { return nil }
        defer { CVPixelBufferUnlockBaseAddress(buffer, .readOnly) }
        let width = CVPixelBufferGetWidth(buffer), height = CVPixelBufferGetHeight(buffer)
        guard let base = CVPixelBufferGetBaseAddress(buffer) else { return nil }
        let rowBytes = CVPixelBufferGetBytesPerRow(buffer)
        let palette: [(UInt8, UInt8, UInt8)] = (0..<256).map { index in
            let hue = CGFloat(index) / 255 * 0.72
            let color = UIColor(hue: hue, saturation: 0.92, brightness: 1, alpha: 1)
            var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
            color.getRed(&r, green: &g, blue: &b, alpha: &a)
            return (UInt8(r * 255), UInt8(g * 255), UInt8(b * 255))
        }
        var pixels = [UInt8](repeating: 0, count: width * height * 4)
        for y in 0..<height {
            let row = UnsafeRawPointer(base.advanced(by: y * rowBytes)).assumingMemoryBound(to: Float32.self)
            for x in 0..<width {
                let d = row[x]
                let i = (y * width + x) * 4
                if d.isFinite && d > 0.12 {
                    let t = min(max((d - 0.25) / 4.0, 0), 1)
                    let color = palette[Int((1 - t) * 255)]
                    pixels[i] = color.0; pixels[i + 1] = color.1
                    pixels[i + 2] = color.2; pixels[i + 3] = 255
                }
            }
        }
        let data = Data(pixels) as CFData
        guard let provider = CGDataProvider(data: data),
              let cg = CGImage(width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32,
                               bytesPerRow: width * 4, space: CGColorSpaceCreateDeviceRGB(),
                               bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
                               provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent) else { return nil }
        return UIImage(cgImage: cg, scale: 1, orientation: orientation)
    }
}

private final class SessionDelegate: NSObject, ARSessionDelegate {
    let model: CameraModel
    init(model: CameraModel) { self.model = model }
    func session(_ session: ARSession, didUpdate frame: ARFrame) {
        Task { @MainActor [weak model] in model?.received(frame) }
    }
}

private struct CameraFeed: UIViewRepresentable {
    @ObservedObject var model: CameraModel
    func makeUIView(context: Context) -> ARSCNView {
        let view = ARSCNView(frame: .zero)
        view.automaticallyUpdatesLighting = false
        view.backgroundColor = UIColor(CameraStyle.ink)
        view.session.delegate = context.coordinator.delegate
        model.cameraView = view
        model.configure(view.session)
        return view
    }
    func updateUIView(_ view: ARSCNView, context: Context) {}
    func makeCoordinator() -> Coordinator { Coordinator(model: model) }
    final class Coordinator {
        let delegate: SessionDelegate
        init(model: CameraModel) { delegate = SessionDelegate(model: model) }
    }
}

private struct LiDARMeshOverlay: View {
    @ObservedObject var model: CameraModel

    var body: some View {
        Canvas { context, _ in
                if model.roomMeshOn {
                    context.stroke(model.roomMeshPath, with: .color(.mint.opacity(0.52)), lineWidth: 0.8)
                }
                guard model.vectorsOn && model.hasLiDAR else { return }
                for segment in model.meshLines {
                    var path = Path()
                    path.move(to: segment.start)
                    path.addLine(to: segment.end)
                    let fade = max(0.2, 1 - segment.depth / 7)
                    context.stroke(path, with: .color(CameraStyle.lime.opacity(fade * 0.78)),
                                   style: StrokeStyle(lineWidth: 1.15, lineCap: .round))
                }
                for point in model.meshPoints {
                    let dot = CGRect(x: point.x - 1.3, y: point.y - 1.3, width: 2.6, height: 2.6)
                    context.fill(Path(ellipseIn: dot), with: .color(.white.opacity(0.72)))
                }
        }
        .allowsHitTesting(false)
    }
}

private struct CameraFrustumOverlay: View {
    @ObservedObject var model: CameraModel
    var body: some View {
        Canvas { context, _ in
            guard model.fovOn, let center = model.fovLines.first?.start else { return }
            for ray in model.fovLines {
                var path = Path()
                path.move(to: ray.start)
                path.addLine(to: ray.end)
                context.stroke(path, with: .color(Color.cyan.opacity(0.42)),
                               style: StrokeStyle(lineWidth: 0.8, dash: [4, 5]))
                let marker = CGRect(x: ray.end.x - 2, y: ray.end.y - 2, width: 4, height: 4)
                context.fill(Path(ellipseIn: marker), with: .color(.cyan.opacity(0.72)))
            }
            let marker = CGRect(x: center.x - 4, y: center.y - 4, width: 8, height: 8)
            context.stroke(Path(ellipseIn: marker), with: .color(.cyan), lineWidth: 1)
        }.allowsHitTesting(false)
    }
}

private struct ShareSheet: UIViewControllerRepresentable {
    let archive: URL
    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: [archive], applicationActivities: nil)
    }
    func updateUIViewController(_ controller: UIActivityViewController, context: Context) {}
}

struct ContentView: View {
    @Environment(\.scenePhase) private var scenePhase
    @StateObject private var model = CameraModel()
    @State private var showLibrary = false
    @State private var showSettings = false

    var body: some View {
        GeometryReader { proxy in
            let compact = proxy.size.height < 500
            ZStack {
                CameraStyle.ink.ignoresSafeArea()
                CameraFeed(model: model).ignoresSafeArea()
                LiDARMeshOverlay(model: model).ignoresSafeArea()
                CameraFrustumOverlay(model: model).ignoresSafeArea()
                LinearGradient(colors: [.black.opacity(0.55), .clear, .clear, .black.opacity(0.82)],
                               startPoint: .top, endPoint: .bottom).ignoresSafeArea()
                    .allowsHitTesting(false)
                if model.gridOn { compositionGrid }
                VStack(spacing: 0) {
                    header
                    Spacer(minLength: 8)
                    if model.showDepth { depthCard }
                    bottomControls(compact: compact)
                }
                if let toast = model.toast { toastView(toast).transition(.move(edge: .top).combined(with: .opacity)).zIndex(5) }
            }
            .preferredColorScheme(.dark)
            .onChange(of: scenePhase, initial: true) { _, phase in
                model.setCaptureInterfaceActive(phase == .active && !showLibrary)
            }
            .onChange(of: showLibrary) { _, open in model.setCaptureInterfaceActive(scenePhase == .active && !open) }
            .onDisappear { model.setCaptureInterfaceActive(false) }
            .sheet(isPresented: $showLibrary) { librarySheet }
            .confirmationDialog("Camera settings", isPresented: $showSettings, titleVisibility: .visible) {
                Button(model.gridOn ? "Hide grid" : "Show grid") { model.gridOn.toggle() }
                Button(model.vectorsOn ? "Hide LiDAR vectors" : "Show LiDAR vectors") { model.vectorsOn.toggle() }
                Button(model.roomMeshOn ? "Hide reconstructed mesh" : "Show reconstructed mesh") { model.roomMeshOn.toggle() }
                Button(model.fovOn ? "Hide FOV rays" : "Show FOV rays") { model.fovOn.toggle() }
                Button(model.showDepth ? "Hide depth map" : "Show depth map") { model.showDepth.toggle() }
                Button("Cancel", role: .cancel) {}
            }
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .top) {
                VStack(alignment: .leading, spacing: 4) {
                    Label("FIELDNOTE", systemImage: "viewfinder")
                        .font(.system(size: 13, weight: .bold, design: .rounded))
                        .tracking(2.1)
                    statusPill
                }
                Spacer()
                Button { showSettings = true } label: {
                    Image(systemName: "slider.horizontal.3").font(.system(size: 17, weight: .medium))
                        .foregroundStyle(.white)
                        .frame(width: 44, height: 44)
                        .background(.ultraThinMaterial, in: Circle())
                }.accessibilityLabel("Camera settings")
            }
            telemetryCard
        }
        .padding(.horizontal, 24)
        .padding(.top, 8)
    }

    private var telemetryCard: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(String(format: "HFOV %4.1f°   VFOV %4.1f°", model.horizontalFOV, model.verticalFOV))
            Text(model.poseReadout)
            Text(model.angleReadout)
            Text("\(model.aimReadout) · \(model.exposureReadout)")
            HStack(spacing: 8) {
                Text(model.ambientReadout)
                Text("MESH \(model.meshFaceCount)F")
                Text("FEATURES \(model.featurePointCount)")
            }
        }
        .font(.system(size: 8, weight: .medium, design: .monospaced))
        .foregroundStyle(.white.opacity(0.78))
        .padding(.horizontal, 10).padding(.vertical, 8)
        .background(.black.opacity(0.42), in: RoundedRectangle(cornerRadius: 9))
        .overlay(RoundedRectangle(cornerRadius: 9).stroke(.white.opacity(0.12), lineWidth: 1))
    }

    private var statusPill: some View {
        HStack(spacing: 7) {
            Circle().fill(model.hasLiDAR ? CameraStyle.lime : .orange).frame(width: 6, height: 6)
            Text(model.hasLiDAR ? model.trackingText.uppercased() : "LIDAR NOT DETECTED")
                .font(.system(size: 9, weight: .semibold, design: .monospaced)).tracking(1)
            Rectangle().fill(.white.opacity(0.22)).frame(width: 1, height: 12)
            Image(systemName: "cube.transparent").font(.system(size: 10))
            Text("3D").font(.system(size: 9, weight: .bold, design: .monospaced))
        }.foregroundStyle(.white).padding(.horizontal, 11).padding(.vertical, 8)
            .background(.black.opacity(0.52), in: Capsule()).overlay(Capsule().stroke(.white.opacity(0.15), lineWidth: 1))
    }

    private var compositionGrid: some View {
        GeometryReader { geo in
            Path { path in
                for fraction in [1.0 / 3.0, 2.0 / 3.0] {
                    let x = geo.size.width * fraction, y = geo.size.height * fraction
                    path.move(to: CGPoint(x: x, y: 0)); path.addLine(to: CGPoint(x: x, y: geo.size.height))
                    path.move(to: CGPoint(x: 0, y: y)); path.addLine(to: CGPoint(x: geo.size.width, y: y))
                }
            }.stroke(.white.opacity(0.25), style: StrokeStyle(lineWidth: 0.7, dash: [3, 5]))
        }.padding(.horizontal, 26).padding(.vertical, 22).allowsHitTesting(false)
    }

    private var depthCard: some View {
        HStack(spacing: 12) {
            Group {
                if let image = model.depthImage { Image(uiImage: image).resizable().interpolation(.low).scaledToFit() }
                else { ZStack { Color.white.opacity(0.05); Image(systemName: "sensor") } }
            }.frame(width: 70, height: 54).clipped().clipShape(RoundedRectangle(cornerRadius: 9))
            VStack(alignment: .leading, spacing: 5) {
                Text("LIVE DEPTH MAP").font(.system(size: 9, weight: .bold, design: .monospaced)).tracking(1.1)
                HStack(spacing: 5) {
                    ForEach([Color.blue, .cyan, .green, .yellow, .orange, .red], id: \.self) { color in color.frame(height: 3) }
                }.clipShape(Capsule())
                HStack {
                    Text("NEAR")
                    Spacer()
                    Text("FAR")
                }.font(.system(size: 7, design: .monospaced)).foregroundStyle(CameraStyle.muted)
            }
            Spacer(minLength: 0)
            Button { model.showDepth = false } label: { Image(systemName: "xmark").font(.system(size: 10, weight: .bold)).foregroundStyle(.white.opacity(0.75)).padding(8) }
        }.padding(10).background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 14)).overlay(RoundedRectangle(cornerRadius: 14).stroke(.white.opacity(0.12), lineWidth: 1))
            .frame(maxWidth: 340).padding(.horizontal, 24).padding(.bottom, 6)
    }

    private func bottomControls(compact: Bool) -> some View {
        VStack(spacing: 8) {
            HStack(alignment: .center) {
                Button { showLibrary = true } label: { libraryThumb }
                    .accessibilityLabel("Open scans")
                Spacer()
                Button { model.capture() } label: {
                    ZStack {
                        Circle().stroke(.white.opacity(0.75), lineWidth: 2).frame(width: 74, height: 74)
                        Circle().fill(model.isCapturing ? CameraStyle.lime.opacity(0.6) : .white).frame(width: 60, height: 60)
                        if model.isCapturing { ProgressView().tint(.black).accessibilityLabel("Collecting ambient evidence") }
                    }
                }.disabled(model.isCapturing).accessibilityLabel("Take photo")
                Spacer()
                Button { model.showDepth.toggle() } label: {
                    Image(systemName: model.showDepth ? "sensor.tag.radiowaves.forward.fill" : "sensor.tag.radiowaves.forward")
                        .font(.system(size: 17, weight: .medium)).foregroundStyle(model.showDepth ? CameraStyle.lime : .white)
                        .frame(width: 48, height: 48).background(.ultraThinMaterial, in: Circle()).overlay(Circle().stroke(.white.opacity(0.16), lineWidth: 1))
                }.accessibilityLabel("Toggle depth map")
            }.padding(.horizontal, 34)
            if !compact {
                Text("CAPTURE PHOTO + 3D POINT CLOUD")
                    .font(.system(size: 8, weight: .medium, design: .monospaced))
                    .tracking(1.1).foregroundStyle(.white.opacity(0.6))
            }
        }
        .padding(.vertical, compact ? 4 : 12)
        .background(LinearGradient(colors: [.clear, .black.opacity(0.24), .black.opacity(0.75)], startPoint: .top, endPoint: .bottom))
    }

    private var libraryThumb: some View {
        Group {
            if let capture = model.captures.first, let image = UIImage(contentsOfFile: capture.url.path) {
                Image(uiImage: image).resizable().scaledToFill()
            } else {
                ZStack { CameraStyle.panel; Image(systemName: "square.stack").font(.system(size: 17)).foregroundStyle(.white.opacity(0.8)) }
            }
        }.frame(width: 47, height: 47).clipShape(RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).stroke(.white.opacity(0.6), lineWidth: 1.5))
            .overlay(alignment: .bottomTrailing) {
                if !model.captures.isEmpty { Circle().fill(CameraStyle.lime).frame(width: 13, height: 13).overlay(Text("\(min(model.captures.count, 9))").font(.system(size: 8, weight: .bold)).foregroundStyle(.black)).offset(x: 3, y: 3) }
            }
    }

    private var librarySheet: some View {
        NavigationStack {
            Group {
                if model.captures.isEmpty {
                    VStack(spacing: 12) { Image(systemName: "viewfinder").font(.system(size: 34)).foregroundStyle(CameraStyle.lime); Text("Your scans appear here").font(.headline); Text("Capture a photo to start your spatial library.").font(.subheadline).foregroundStyle(.secondary).multilineTextAlignment(.center) }.padding(32)
                } else {
                    ScrollView { LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 12) {
                        ForEach(model.captures) { capture in
                            NavigationLink { CapturePreview(capture: capture) } label: {
                                VStack(alignment: .leading, spacing: 6) {
                                    if let image = UIImage(contentsOfFile: capture.url.path) { Image(uiImage: image).resizable().scaledToFill().frame(height: 145).frame(maxWidth: .infinity).clipped().clipShape(RoundedRectangle(cornerRadius: 12)) }
                                    HStack(spacing: 5) {
                                        Text(capture.date.formatted(date: .abbreviated, time: .shortened)).font(.caption2).foregroundStyle(.secondary)
                                        Spacer(minLength: 0)
                                        if capture.jsonURL != nil { Label("3D", systemImage: "point.3.connected.trianglepath.dotted").font(.system(size: 8, weight: .bold)).foregroundStyle(CameraStyle.lime) }
                                    }
                                }
                            }.buttonStyle(.plain)
                        }
                    }.padding() }
                }
            }.navigationTitle("Scans · \(model.captures.count)").navigationBarTitleDisplayMode(.inline)
                .toolbar { ToolbarItem(placement: .topBarTrailing) { Button("Done") { showLibrary = false } } }
        }.presentationDetents([.medium, .large]).presentationDragIndicator(.visible)
    }

    private func toastView(_ text: String) -> some View {
        Text(text).font(.system(size: 13, weight: .semibold)).foregroundStyle(.white).padding(.horizontal, 16).padding(.vertical, 11)
            .background(.ultraThinMaterial, in: Capsule()).overlay(Capsule().stroke(.white.opacity(0.15), lineWidth: 1)).padding(.top, 105)
            .frame(maxHeight: .infinity, alignment: .top)
    }
}

private struct CapturePreview: View {
    let capture: Capture
    @State private var showShare = false
    @State private var archive: URL?
    @State private var preparingArchive = false
    @State private var exportFailed = false
    @State private var summary: CaptureProofSummary?
    @State private var receipt: AnchorReceipt?
    @State private var networkStatus = "Unverified"

    var body: some View {
        ScrollView {
            VStack(spacing: 14) {
                if let image = UIImage(contentsOfFile: capture.url.path) {
                    Image(uiImage: image).resizable().scaledToFit()
                }
                CaptureProofView(summary: summary, receipt: receipt, networkStatus: networkStatus)
                if summary?.proof != nil, XRPLAnchorService.configured != nil {
                    Button("Refresh verification", action: refresh)
                }
            }.padding()
        }
        .task { await verifyCapture() }
        .navigationTitle(capture.date.formatted(date: .abbreviated, time: .shortened))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                if preparingArchive {
                    ProgressView().accessibilityLabel("Preparing full scan")
                } else {
                    Menu {
                        Button("Share full scan", systemImage: "archivebox") { prepareArchive() }
                        if let proof = summary?.proof, summary?.integrity == true {
                            Section("Selective disclosure") {
                                ForEach(proof.disclosures, id: \.category) { entry in
                                    Button(entry.category) { prepareArchive(disclosure: entry, root: proof.root) }
                                }
                            }
                        }
                    } label: {
                        Label("Share scan", systemImage: "square.and.arrow.up")
                    }
                }
            }
        }
        .sheet(isPresented: $showShare, onDismiss: removeArchive) {
            if let archive { ShareSheet(archive: archive) }
        }
        .alert("Could not prepare the scan ZIP", isPresented: $exportFailed) {
            Button("OK", role: .cancel) {}
        }
        .navigationBarBackButtonHidden(preparingArchive)
        .interactiveDismissDisabled(preparingArchive)
    }

    private func refresh() {
        Task { await verifyCapture() }
    }

    private func verifyCapture() async {
        let proof = capture.evidenceURL("proof.json")
        let radio = capture.evidenceURL("radio.json")
        let spatial = capture.evidenceURL("spatial.json")
        let imu = capture.evidenceURL("imu.json")
        let noise = capture.evidenceURL("noise.json")
        let report = await Task.detached(priority: .userInitiated) {
            CaptureProofSummary.load(proofURL: proof, radioURL: radio, spatialURL: spatial, imuURL: imu, noiseURL: noise)
        }.value
        summary = report
        receipt = nil
        guard report.integrity, let root = report.proof?.root else { return }
        guard let backend = XRPLAnchorService.configured else {
            networkStatus = "Backend not configured"
            return
        }
        networkStatus = "Checking…"
        receipt = try? await backend.status(root: root)
        networkStatus = receipt == nil ? "Unverified" : "Not yet validated"
    }

    private func prepareArchive(disclosure: EvidenceCommitment.Disclosure? = nil, root: String? = nil) {
        guard !preparingArchive else { return }
        preparingArchive = true
        let folder = capture.url.deletingLastPathComponent()
        let proofFile = capture.evidenceURL("proof.json")
        let files = [capture.url, capture.jsonURL, capture.depthURL, capture.nativeFrameURL].compactMap { $0 } + capture.extraEvidence
        let name = capture.url.deletingPathExtension().lastPathComponent
        Task {
            defer { preparingArchive = false }
            do {
                archive = try await Task.detached(priority: .userInitiated) {
                    if let disclosure, let root {
                        guard EvidenceVerifier.verifies(disclosure, root: root, folder: folder) else {
                            throw CocoaError(.fileReadCorruptFile)
                        }
                        let selected = disclosure.fileHashes.keys.sorted().map { folder.appendingPathComponent($0) }
                        let proof = EvidenceCommitment(protocolVersion: 1, root: root, disclosures: [disclosure])
                        let bytes = try EvidenceCommitmentBuilder.canonical(proof)
                        return try ScanExport.makeArchive(files: selected, name: "\(name)-\(disclosure.category)", disclosure: bytes)
                    }
                    if FileManager.default.fileExists(atPath: proofFile.path) {
                        let proof = try JSONDecoder().decode(EvidenceCommitment.self, from: Data(contentsOf: proofFile))
                        guard EvidenceVerifier.verifiesAll(proof, folder: folder) else { throw CocoaError(.fileReadCorruptFile) }
                        let filenames = Set(proof.disclosures.flatMap { Array($0.fileHashes.keys) })
                        let committed = filenames.sorted().map { folder.appendingPathComponent($0) }
                        return try ScanExport.makeArchive(files: committed + [proofFile], name: name)
                    }
                    return try ScanExport.makeArchive(files: files, name: name)
                }.value
                showShare = true
            } catch {
                exportFailed = true
            }
        }
    }

    private func removeArchive() {
        if let archive { ScanExport.removeArchive(archive) }
        archive = nil
    }
}

#Preview { ContentView() }
