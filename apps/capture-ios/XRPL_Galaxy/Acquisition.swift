import ARKit
import AVFoundation
import Foundation
import Observation
import QuartzCore
import SceneKit
import simd

@MainActor
@Observable
final class Acquisition: NSObject, @preconcurrency ARSessionDelegate {
    private struct Pending {
        let center: FrameSnapshot
        let minus: FrameSnapshot?
        var plus: FrameSnapshot?
    }

    let preview = ARSCNView(frame: .zero)
    let overlay = OverlayModel()
    var record: CaptureRecord?
    private(set) var cameraStatus = "Waiting for camera"
    private(set) var videoFormat = "Not selected"
    private(set) var imuStatus = "Not collecting"
    private(set) var bluetoothStatus = "Not scanning"
    private(set) var depthCoverage: Double?
    /// Camera speed and rotation rate over the last ~0.45 s (HUD guidance).
    private(set) var speedMmPerSecond: Double?
    private(set) var rotationDegPerSecond: Double?
    private(set) var trackingNormal = false
    private(set) var certificationTrackingReady = false
    private(set) var certificationDepthReady = false
    private(set) var attempts = 0
    private(set) var exported = 0
    /// Experiment label for the next attempt; persisted so a session of one scene class needs no re-selection.
    var sceneLabel: SceneLabel = SceneLabel(rawValue: UserDefaults.standard.string(forKey: "galaxy.sceneLabel") ?? "") ?? .unlabelled {
        didSet { UserDefaults.standard.set(sceneLabel.rawValue, forKey: "galaxy.sceneLabel") }
    }
    private(set) var clockSummary: String?
    private(set) var notice: String?
    private(set) var lastRaw: CaptureRecord?
    private(set) var running = false
    private(set) var writing = false
    var calibrationMode = false
    var autoExportFixtures = false
    var policyLimits: CapturePolicyLimits?
    var isStaticDiagnostic: Bool { staticProbe }
    var isCalibration: Bool { staticProbe || calibrationMode }
    @ObservationIgnored var onFinished: ((CaptureRecord) -> Void)?
    @ObservationIgnored var onStorageFailure: ((CaptureRecord) -> Void)?

    @ObservationIgnored private let motion = MotionWindow()
    @ObservationIgnored private let bluetooth = BluetoothWindow()
    @ObservationIgnored private var store: CaptureStore?
    @ObservationIgnored private var history: [FrameSnapshot] = []
    @ObservationIgnored private var pending: Pending?
    @ObservationIgnored private var timer: Task<Void, Never>?
    @ObservationIgnored private var active = false
    @ObservationIgnored private var requestingPermission = false
    @ObservationIgnored private var firstFrameTime: Double?
    @ObservationIgnored private var lastTrackingFrame: Double?
    @ObservationIgnored private var normalTrackingSince: Double?
    @ObservationIgnored private var policyDepthSince: Double?
    @ObservationIgnored private var probeTriggered = false
    @ObservationIgnored private var attempt: (label: SceneLabel, coverage: Double?, orientation: String)?
    @ObservationIgnored private var attemptMotion = MotionSummary()
    /// Set by the shutter: the next delivered frame becomes t0 (watchdog fails the attempt after 0.35 s).
    @ObservationIgnored private var centerDeadline: Double?
    private let staticProbe: Bool

    override init() {
        #if DEBUG
        staticProbe = ProcessInfo.processInfo.arguments.contains("--static-capture-probe")
        #else
        staticProbe = false
        #endif
        super.init()
        preview.session.delegateQueue = .main
        preview.session.delegate = self
        preview.isUserInteractionEnabled = false
        preview.automaticallyUpdatesLighting = false
    }

    var canCapture: Bool { running && !writing && pending == nil && centerDeadline == nil && store != nil }

    var canCertify: Bool { canCapture && certificationTrackingReady && certificationDepthReady }

    func setActive(_ value: Bool) {
        active = value
        guard value else { suspend(); return }
        if store == nil {
            do {
                let opened = try CaptureStore()
                let records = try opened.recover()
                record = records.first
                lastRaw = records.first(where: opened.hasRaw)
                store = opened
                notice = nil
            } catch {
                notice = "Protected capture storage could not be read. Unlock the device and reopen the app."
            }
        }
        guard !running, !requestingPermission else { return }
        guard ARWorldTrackingConfiguration.isSupported,
              ARWorldTrackingConfiguration.supportsFrameSemantics(.sceneDepth) else {
            cameraStatus = CaptureIssue.cameraUnavailable.message
            return
        }
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized: start()
        case .notDetermined:
            requestingPermission = true
            Task {
                let granted = await AVCaptureDevice.requestAccess(for: .video)
                requestingPermission = false
                if granted && active { start() }
                else if !granted { cameraStatus = "Camera permission was denied. Enable it in Settings." }
            }
        case .denied, .restricted: cameraStatus = "Camera permission is unavailable. Enable it in Settings."
        @unknown default: cameraStatus = CaptureIssue.cameraUnavailable.message
        }
    }

    private func start() {
        let formats = ARWorldTrackingConfiguration.supportedVideoFormats
        let describe: (ARConfiguration.VideoFormat) -> String = {
            "\(Int($0.imageResolution.width))×\(Int($0.imageResolution.height)) @ \($0.framesPerSecond) fps (\($0.captureDeviceType.rawValue))"
        }
        print("GXCAPTURE supported formats: \(formats.map(describe).joined(separator: "; "))")
        let matching = formats.filter {
            $0.imageResolution.width == CGFloat(FrameSnapshot.width) && $0.imageResolution.height == CGFloat(FrameSnapshot.height)
                && $0.captureDeviceType == .builtInWideAngleCamera && $0.captureDevicePosition == .back
        }
        guard let format = matching.first(where: { $0.framesPerSecond == 30 }) ?? matching.first else {
            cameraStatus = CaptureIssue.imageFormat.message
            return
        }
        let config = ARWorldTrackingConfiguration()
        config.videoFormat = format
        config.frameSemantics = .sceneDepth
        config.planeDetection = []
        config.sceneReconstruction = []
        config.environmentTexturing = .none
        config.worldAlignment = .gravity
        videoFormat = describe(format)
        print("GXCAPTURE selected: \(videoFormat)")
        cameraStatus = "Starting unsmoothed LiDAR"
        running = true
        motion.start()
        bluetooth.start()
        preview.session.run(config, options: [.resetTracking, .removeExistingAnchors])
    }

    private func suspend() {
        guard running else { return }
        running = false
        preview.session.pause()
        motion.stop()
        bluetooth.stop()
        history.removeAll(keepingCapacity: true)
        firstFrameTime = nil
        lastTrackingFrame = nil
        normalTrackingSince = nil
        policyDepthSince = nil
        trackingNormal = false
        certificationTrackingReady = false
        certificationDepthReady = false
        depthCoverage = nil
        speedMmPerSecond = nil
        rotationDegPerSecond = nil
        overlay.reset()
        imuStatus = "Not collecting"
        bluetoothStatus = "Not scanning"
        if pending != nil || centerDeadline != nil {
            timer?.cancel()
            pending = nil
            centerDeadline = nil
            finishWithoutRaw(["Acquisition was interrupted before the shutter window completed."])
        }
    }

    func session(_ session: ARSession, didUpdate frame: ARFrame) {
        guard running else { return }
        let timestamp = frame.timestamp
        guard timestamp.isFinite, timestamp > 0 else { return }
        updateTrackingReadiness(frame.camera.trackingState, at: timestamp)
        // The shutter takes the next delivered frame as t0. Otherwise keep at most ~15 fps
        // (every second 30 fps frame despite timestamp jitter).
        let wantsCenter = centerDeadline != nil
        if !wantsCenter, let previous = history.last, timestamp - previous.timestamp < 0.06 { return }
        let snapshot = FrameSnapshot(frame, arrival: CACurrentMediaTime())
        if !isCalibration, let policyLimits, let depth = snapshot.depth, depth.hasPolicyDepth(policyLimits) {
            if policyDepthSince == nil { policyDepthSince = timestamp }
        } else { policyDepthSince = nil }
        let depthReady = policyDepthSince.map { timestamp - $0 >= 1 } ?? false
        if certificationDepthReady != depthReady { certificationDepthReady = depthReady }
        history.removeAll { timestamp - $0.timestamp > 0.6 }
        if history.count == 10 { history.removeFirst() }
        let previous = nearest(in: history, target: timestamp - 0.45, lower: timestamp - 0.55, upper: timestamp - 0.3)
        if let previous, timestamp > previous.timestamp {
            let dt = timestamp - previous.timestamp
            speedMmPerSecond = Self.baseline(snapshot, previous) * 1_000 / dt
            rotationDegPerSecond = PoseMath.rotationDeg(previous.transform, snapshot.transform) / dt
        } else { speedMmPerSecond = nil; rotationDegPerSecond = nil }
        history.append(snapshot)
        trackingNormal = snapshot.tracking == 2
        cameraStatus = snapshot.tracking == 2 ? "Tracking normal" : "Tracking: \(snapshot.trackingReason)"
        depthCoverage = snapshot.depth?.coverage()
        imuStatus = motion.status
        bluetoothStatus = bluetooth.status
        let orientation = preview.window?.windowScene?.effectiveGeometry.interfaceOrientation ?? .landscapeRight
        if isCalibration { overlay.update(frame: frame, snapshot: snapshot, viewport: preview.bounds.size, orientation: orientation) }
        let wanted: SCNDebugOptions = isCalibration && overlay.showFeatures ? [ARSCNDebugOptions.showFeaturePoints] : []
        if preview.debugOptions != wanted { preview.debugOptions = wanted }
        if wantsCenter {
            centerDeadline = nil
            pending = Pending(center: snapshot,
                              minus: widest(in: history, center: snapshot,
                                            lower: timestamp - 0.5, upper: timestamp - 0.1))
            // Consider the entire successor window, then allow an additional 50 ms of motion coverage.
            let delay = max(0, timestamp + 0.58 - CACurrentMediaTime())
            timer = Task { [weak self] in
                do { try await Task.sleep(for: .seconds(delay)) } catch { return }
                await self?.freeze()
            }
        } else if var window = pending {
            let delta = timestamp - window.center.timestamp
            // Within the allowed 100–500 ms window favor usable frames and larger displacement.
            if (0.1...0.5).contains(delta),
               window.plus.map({ Self.score($0, center: window.center) < Self.score(snapshot, center: window.center) }) ?? true {
                window.plus = snapshot
                pending = window
            }
        }
        if firstFrameTime == nil { firstFrameTime = timestamp }
        // Diagnostic trigger: after 3 s, once every frame of the 0.6 s history tracks normally
        // (so t− can be normal too); after 15 s regardless, to record the failure reasons.
        let settled = history.count >= 8 && history.allSatisfy { $0.tracking == 2 }
        if staticProbe && !probeTriggered && canCapture, let first = firstFrameTime,
           (timestamp - first >= 3 && settled || timestamp - first >= 15) {
            probeTriggered = true
            capture()
        }
    }

    private func updateTrackingReadiness(_ state: ARCamera.TrackingState, at timestamp: Double) {
        if let previous = lastTrackingFrame, timestamp <= previous || timestamp - previous > 0.25 {
            normalTrackingSince = nil
            policyDepthSince = nil
            certificationDepthReady = false
        }
        lastTrackingFrame = timestamp
        if case .normal = state {
            if normalTrackingSince == nil { normalTrackingSince = timestamp }
        } else { normalTrackingSince = nil }
        let trackingReady = normalTrackingSince.map { timestamp - $0 >= 1 } ?? false
        if certificationTrackingReady != trackingReady { certificationTrackingReady = trackingReady }
    }

    func session(_ session: ARSession, didFailWithError error: Error) {
        suspend()
        let failure = error as NSError
        cameraStatus = "ARKit session failed (\(failure.domain) \(failure.code))."
    }

    func sessionWasInterrupted(_ session: ARSession) {
        suspend()
        cameraStatus = "ARKit session interrupted"
    }

    func sessionInterruptionEnded(_ session: ARSession) {
        if active && !running { setActive(true) }
    }

    func capture(certification: CertificationJournal? = nil) {
        guard let store, certification != nil ? canCertify : canCapture && isCalibration else { return }
        var row = CaptureRecord(id: UUID(), claimedTimeMs: UInt64((Date().timeIntervalSince1970 * 1000).rounded()))
        row.certification = certification
        notice = nil
        do {
            try row.transition(to: .acquiring)
            try store.save(row)
            record = row
            attempts += 1
            attempt = (sceneLabel, depthCoverage, Self.orientationName(preview.window?.windowScene?.effectiveGeometry.interfaceOrientation))
            attemptMotion = MotionSummary()
        } catch {
            notice = "Storage error"
            return
        }
        // t0 is the first frame delivered after the press (see session(_:didUpdate:)).
        let deadline = CACurrentMediaTime() + 0.3
        centerDeadline = deadline
        timer = Task { [weak self] in
            do { try await Task.sleep(for: .seconds(0.35)) } catch { return }
            guard let self, self.centerDeadline == deadline else { return }
            self.centerDeadline = nil
            self.finishWithoutRaw([CaptureIssue.missingFrame.message])
        }
    }

    /// Neighbour ranking: frames with normal tracking, pixels and depth first, then larger baseline.
    private static func score(_ frame: FrameSnapshot, center: FrameSnapshot) -> (Int, Double) {
        (frame.tracking == 2 && frame.depth != nil && frame.pixels != nil ? 1 : 0, baseline(frame, center))
    }

    private func widest(in candidates: [FrameSnapshot], center: FrameSnapshot, lower: Double, upper: Double) -> FrameSnapshot? {
        candidates.lazy.filter { (lower...upper).contains($0.timestamp) }
            .max(by: { Self.score($0, center: center) < Self.score($1, center: center) })
    }

    private func nearest(in candidates: [FrameSnapshot], target: Double, lower: Double, upper: Double) -> FrameSnapshot? {
        candidates.lazy.filter { (lower...upper).contains($0.timestamp) }
            .min(by: { abs($0.timestamp - target) < abs($1.timestamp - target) })
    }

    private static func baseline(_ a: FrameSnapshot, _ b: FrameSnapshot) -> Double {
        (Double(simd_length(a.position - b.position)) * 1_000_000).rounded() / 1_000_000
    }

    /// Standalone calibration may run without a configured circuit; never used for certification.
    private static let diagnosticBaselineMinMeters = 0.0005

    private static func rotationSkew(_ first: FrameSnapshot, _ center: FrameSnapshot) -> Double {
        func rotation(_ frame: FrameSnapshot) -> simd_float3x3 {
            let m = frame.transform
            return .init(SIMD3(m.columns.0.x, m.columns.0.y, m.columns.0.z),
                         SIMD3(m.columns.1.x, m.columns.1.y, m.columns.1.z),
                         SIMD3(m.columns.2.x, m.columns.2.y, m.columns.2.z))
        }
        let r = rotation(first).transpose * rotation(center)
        let skew = [r.columns.1.z - r.columns.2.y, r.columns.2.x - r.columns.0.z, r.columns.0.y - r.columns.1.x]
        guard skew.allSatisfy(\.isFinite) else { return .infinity }
        return Double(skew.map(abs).max() ?? 0)
    }

    private func freeze() async {
        guard let window = pending, let store, var row = record, row.state == .acquiring else { return }
        pending = nil
        attemptMotion = Self.motionSummary(window)
        if row.certification != nil {
            row.frameTracking = .init(minus: window.minus?.trackingReason, center: window.center.trackingReason,
                                      plus: window.plus?.trackingReason)
            record = row
        }
        let bluetooth = bluetooth.freeze(center: window.center.timestamp, until: CACurrentMediaTime())
        guard let minus = window.minus, let plus = window.plus else {
            var reasons = [CaptureIssue.missingFrame.message]
            if window.minus == nil { reasons.append("No frame 100–500 ms before the shutter.") }
            if window.plus == nil { reasons.append("No frame 100–500 ms after the shutter.") }
            finishWithoutRaw(reasons)
            return
        }
        let frames = [minus, window.center, plus]
        var issues: [CaptureIssue] = []
        if frames.contains(where: { $0.tracking != 2 }) { issues.append(.tracking) }
        if frames.contains(where: { $0.pixels == nil }) { issues.append(.imageFormat) }
        if frames.contains(where: { $0.depth == nil }) { issues.append(.missingDepth) }
        let limits = row.certification == nil ? nil : policyLimits
        let baselineFloor = limits.map { Double($0.baselineMinUm) / 1_000_000 } ?? Self.diagnosticBaselineMinMeters
        let minBaseline = min(Self.baseline(minus, window.center), Self.baseline(plus, window.center))
        if !minBaseline.isFinite || minBaseline < baselineFloor { issues.append(.noMotion) }
        if let limits {
            // A conservative sensor preflight, not a policy verdict: no challenge tiles or
            // provisional witness. Check the full center depth plane and the two relative poses.
            if let depth = window.center.depth, !depth.hasPolicyDepth(limits) { issues.append(.noUsableDepth) }
            let skewMax = Double(limits.rotationMaxQ30) / 1_073_741_824 + 0.000_01
            if [minus, plus].contains(where: { Self.rotationSkew($0, window.center) > skewMax }) {
                issues.append(.excessRotation)
            }
        }
        let samples = motion.snapshot(start: minus.timestamp, end: plus.timestamp)
        if samples == nil { issues.append(.motionUnavailable) }
        guard !issues.contains(.imageFormat), !issues.contains(.missingDepth), let samples else {
            finishWithoutRaw(issues.map(\.message)); return
        }
        let nearestSample = samples.min { abs($0.timestamp - window.center.timestamp) < abs($1.timestamp - window.center.timestamp) }!
        let ms = (nearestSample.timestamp - window.center.timestamp) * 1000
        let arLag = window.center.arrivalLag * 1000
        let imuLag = nearestSample.arrivalLag * 1000
        let clock = String(format: "Nearest IMU − AR: %+.2f ms; callback lag AR %.2f ms / IMU %.2f ms", ms, arLag, imuLag)
        clockSummary = clock
        if staticProbe { print("GXCAPTURE \(clock)") }
        writing = true
        defer { writing = false }
        let device = RawCaptureWriter.Device.current()
        let reasons = issues.map(\.message)
        do {
            try row.prepareAcquisition(reasons: reasons)
            try store.save(row)
            let frozen = row
            let capturedIssues = issues
            try await Task.detached(priority: .userInitiated) {
                try RawCaptureWriter.write(frames: frames, motion: samples, bluetooth: bluetooth, device: device,
                                           record: frozen, issues: capturedIssues, to: store.url(frozen))
            }.value
        } catch {
            finishWithoutRaw(["Could not freeze the raw capture in protected storage."], state: .failed)
            return
        }
        do {
            try row.transition(to: issues.isEmpty ? .canonicalizing : .inconclusive, reasons: reasons)
            try store.save(row)
            record = row
            lastRaw = row
        } catch {
            notice = "The capture was frozen. Reload setup to recover its protected record."
            if let record { onStorageFailure?(record) }
            return
        }
        publishAttempt(row, rawAvailable: true)
        onFinished?(row)
        if staticProbe {
            print("GXCAPTURE \(row.state.rawValue): \(row.reasons.joined(separator: "; ")); \(notice ?? "no export result")")
            exitDiagnostic()
        }
    }

    private static func motionSummary(_ window: Pending) -> MotionSummary {
        var summary = MotionSummary()
        let center = window.center.transform
        if let minus = window.minus {
            summary.baselineMinusMm = PoseMath.baselineMm(minus.transform, center)
            summary.rotationMinusDeg = PoseMath.rotationDeg(minus.transform, center)
        }
        if let plus = window.plus {
            summary.baselinePlusMm = PoseMath.baselineMm(plus.transform, center)
            summary.rotationPlusDeg = PoseMath.rotationDeg(plus.transform, center)
        }
        return summary
    }

    /// Exports the frozen raw capture (if any) and writes the label sidecar of the attempt to
    /// Documents/galaxy-fixtures, for every attempt, so experiment statistics include failures.
    private func publishAttempt(_ row: CaptureRecord, rawAvailable: Bool) {
        guard let current = attempt else { return }
        attempt = nil
        guard let store, staticProbe || isCalibration && autoExportFixtures else { return }
        var rawExported = false
        if rawAvailable {
            do {
                try store.export(row)
                rawExported = true
                exported += 1
            } catch CaptureStore.Failure.alreadyExported {
                rawExported = true
            } catch {
                notice = "Export failed"
            }
        }
        let label = CaptureLabel(captureId: row.id, sceneClass: current.label, claimedTimeMs: row.claimedTimeMs,
                                 state: row.state, reasons: row.reasons, rawExported: rawExported,
                                 motion: attemptMotion, depthCoverage: current.coverage,
                                 uiOrientation: current.orientation,
                                 appBuild: Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "unknown")
        attemptMotion = MotionSummary()
        do {
            try store.writeLabel(label)
        } catch {
            notice = "Label write failed"
        }
    }

    private static func orientationName(_ orientation: UIInterfaceOrientation?) -> String {
        switch orientation {
        case .portrait: "portrait"
        case .portraitUpsideDown: "portrait-upside-down"
        case .landscapeLeft: "landscape-left"
        case .landscapeRight: "landscape-right"
        default: "unknown"
        }
    }

    private func finishWithoutRaw(_ reasons: [String], state: CaptureState = .inconclusive) {
        guard let store, var row = record else { return }
        do {
            try row.transition(to: state, reasons: reasons)
            try store.save(row)
            record = row
        } catch {
            notice = "Storage error. Reload setup to recover the protected record."
            if let record { onStorageFailure?(record) }
        }
        if let row = record, row.state == state {
            publishAttempt(row, rawAvailable: false)
            onFinished?(row)
        }
        if staticProbe {
            print("GXCAPTURE \(state.rawValue): \(reasons.joined(separator: "; ")); no raw capture")
            exitDiagnostic()
        }
    }

    func exportLastRaw() {
        guard let store, let lastRaw else { return }
        do {
            try store.export(lastRaw)
            notice = "Exported a private GXRW fixture to Documents/galaxy-fixtures."
        } catch CaptureStore.Failure.alreadyExported {
            notice = CaptureStore.Failure.alreadyExported.errorDescription
        } catch { notice = "Could not export the protected raw capture." }
    }

    private func exitDiagnostic() {
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("--diagnostic-exit") {
            fflush(nil)
            exit(0)
        }
        #endif
    }
}
