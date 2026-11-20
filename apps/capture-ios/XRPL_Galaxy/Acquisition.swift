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
    var record: CaptureRecord?
    private(set) var cameraStatus = "Waiting for camera"
    private(set) var videoFormat = "Not selected"
    private(set) var imuStatus = "Not collecting"
    private(set) var bluetoothStatus = "Not scanning"
    private(set) var depthCoverage: Double?
    private(set) var baselineMillimeters: Double?
    private(set) var clockSummary: String?
    private(set) var notice: String?
    private(set) var lastRaw: CaptureRecord?
    private(set) var running = false
    private(set) var writing = false

    @ObservationIgnored private let motion = MotionWindow()
    @ObservationIgnored private let bluetooth = BluetoothWindow()
    @ObservationIgnored private var store: CaptureStore?
    @ObservationIgnored private var history: [FrameSnapshot] = []
    @ObservationIgnored private var pending: Pending?
    @ObservationIgnored private var timer: Task<Void, Never>?
    @ObservationIgnored private var active = false
    @ObservationIgnored private var requestingPermission = false
    @ObservationIgnored private var firstFrameTime: Double?
    @ObservationIgnored private var probeTriggered = false
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

    var canCapture: Bool { running && !writing && record?.state != .acquiring && store != nil }

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
        depthCoverage = nil
        baselineMillimeters = nil
        imuStatus = "Not collecting"
        bluetoothStatus = "Not scanning"
        if pending != nil {
            timer?.cancel()
            pending = nil
            finishWithoutRaw(["Acquisition was interrupted before the shutter window completed."])
        }
    }

    func session(_ session: ARSession, didUpdate frame: ARFrame) {
        guard running else { return }
        let timestamp = frame.timestamp
        guard timestamp.isFinite, timestamp > 0 else { return }
        // At most ~15 fps: keep every second 30 fps frame despite timestamp jitter.
        if let previous = history.last, timestamp - previous.timestamp < 0.06 { return }
        let snapshot = FrameSnapshot(frame, arrival: CACurrentMediaTime())
        history.removeAll { timestamp - $0.timestamp > 0.6 }
        if history.count == 10 { history.removeFirst() }
        let previous = nearest(in: history, target: timestamp - 0.25, lower: timestamp - 0.5, upper: timestamp - 0.1)
        if let previous {
            baselineMillimeters = Self.baseline(snapshot, previous) * 1_000
        } else { baselineMillimeters = nil }
        history.append(snapshot)
        cameraStatus = snapshot.tracking == 2 ? "Tracking normal" : "Tracking: \(snapshot.trackingReason)"
        depthCoverage = snapshot.depth?.coverage()
        imuStatus = motion.status
        bluetoothStatus = bluetooth.status
        if var window = pending {
            let delta = timestamp - window.center.timestamp
            if (0.1...0.5).contains(delta),
               window.plus.map({ abs(delta - 0.25) < abs($0.timestamp - window.center.timestamp - 0.25) }) ?? true {
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

    func capture() {
        guard canCapture, let store else { return }
        let now = CACurrentMediaTime()
        var row = CaptureRecord(id: UUID(), claimedTimeMs: UInt64((Date().timeIntervalSince1970 * 1000).rounded()))
        notice = nil
        do {
            try store.save(row)
            try row.transition(to: .acquiring)
            try store.save(row)
            record = row
        } catch {
            notice = "Could not save the capture record in protected storage."
            return
        }
        guard let center = history.last, (0...0.12).contains(now - center.timestamp) else {
            finishWithoutRaw([CaptureIssue.missingFrame.message]); return
        }
        pending = Pending(center: center,
                          minus: nearest(in: history, target: center.timestamp - 0.25,
                                         lower: center.timestamp - 0.5, upper: center.timestamp - 0.1))
        // Consider the entire successor window, then allow an additional 50 ms of motion coverage.
        let delay = max(0, center.timestamp + 0.58 - now)
        timer = Task { [weak self] in
            do { try await Task.sleep(for: .seconds(delay)) } catch { return }
            await self?.freeze()
        }
    }

    private func nearest(in candidates: [FrameSnapshot], target: Double, lower: Double, upper: Double) -> FrameSnapshot? {
        candidates.lazy.filter { (lower...upper).contains($0.timestamp) }
            .min(by: { abs($0.timestamp - target) < abs($1.timestamp - target) })
    }

    private static func baseline(_ a: FrameSnapshot, _ b: FrameSnapshot) -> Double {
        (Double(simd_length(a.position - b.position)) * 1_000_000).rounded() / 1_000_000
    }

    private func freeze() async {
        guard let window = pending, let store, var row = record, row.state == .acquiring else { return }
        pending = nil
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
        let minBaseline = min(Self.baseline(minus, window.center), Self.baseline(plus, window.center))
        if !minBaseline.isFinite || minBaseline < 0.003 { issues.append(.insufficientMotion) }
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
            notice = "The capture was frozen. Reopen the app to recover its record."
            return
        }
        if staticProbe {
            exportLastRaw()
            print("GXCAPTURE \(row.state.rawValue): \(row.reasons.joined(separator: "; ")); \(notice ?? "no export result")")
            exitDiagnostic()
        }
    }

    private func finishWithoutRaw(_ reasons: [String], state: CaptureState = .inconclusive) {
        guard let store, var row = record else { return }
        do {
            try row.transition(to: state, reasons: reasons)
            try store.save(row)
            record = row
        } catch { notice = "Could not persist the capture result. Reopen the app to recover it." }
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
