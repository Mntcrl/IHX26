import ARKit
import CryptoKit
import Foundation
import QuartzCore

nonisolated struct AmbientCaptureWindow: Sendable {
    let center: Double
    let date: Date
    let clock: String
    let sessionID: String
    let challenge: CaptureChallenge?
    let preSeconds: Double
    let postSeconds: Double
    let shutterUptime: Double
    var start: Double { center - preSeconds }
    var end: Double { center + postSeconds }
}

nonisolated struct CompletedCaptureWindow: Sendable {
    let window: AmbientCaptureWindow
    let radio: RadioFingerprint
    let poses: [SpatialEvidenceCollector.Pose]
    let trustedAnchors: [TrustedAnchorEvidence]
    let inertial: InertialEvidence
    let cameraReferences: [ARFrame]
}

@MainActor
final class CaptureSessionManager {
    private let scanner = AmbientRadioScanner()
    private let inertial = InertialEvidenceCollector()
    private var referenceHistory: [ARFrame] = []
    private var referenceBefore: ARFrame?
    private var referenceAfter: ARFrame?
    private var referenceCenter: Double?
    private let attestation = XRPLAnchorService.configured.map { CaptureAttestationService(backend: $0) }
    private let trustedAnchors: any TrustedAnchorProvider
    private let trustedKeys: [String: Curve25519.Signing.PublicKey]
    private var poses: [SpatialEvidenceCollector.Pose] = []
    private var active = false
    private var pending = false
    private var challengeTask: Task<Void, Never>?
    private var challenge: CaptureChallenge?
    private var sessionID = UUID().uuidString.lowercased()
    private let preSeconds: Double
    private let postSeconds: Double

    init(trustedAnchors: any TrustedAnchorProvider = NoTrustedAnchors(),
         trustedKeys: [String: Curve25519.Signing.PublicKey] = [:]) {
        self.trustedAnchors = trustedAnchors
        self.trustedKeys = trustedKeys
        let configured = (Bundle.main.object(forInfoDictionaryKey: "AmbientHalfWindowSeconds") as? String).flatMap(Double.init) ?? 5
        preSeconds = configured.isFinite ? min(10, max(1, configured)) : 5
        let post = (Bundle.main.object(forInfoDictionaryKey: "AmbientPostWindowSeconds") as? String).flatMap(Double.init) ?? 0.35
        postSeconds = post.isFinite ? min(5, max(0.25, post)) : 0.35
        scanner.configure(retention: preSeconds + postSeconds + 2)
        inertial.configure(retention: preSeconds + postSeconds + 2)
    }

    func setActive(_ active: Bool) {
        guard active != self.active else { return }
        self.active = active
        scanner.setActive(active)
        inertial.setActive(active, preserveWindow: pending)
        if active {
            prepareChallenge()
        } else {
            challengeTask?.cancel()
            challengeTask = nil
            referenceHistory.removeAll()
            if !pending { poses.removeAll() }
        }
    }

    func record(_ frame: ARFrame) {
        guard active, frame.timestamp.isFinite else { return }
        if let center = referenceCenter {
            if referenceAfter == nil, (min(0.20, postSeconds * 0.65)...postSeconds).contains(frame.timestamp - center) { referenceAfter = frame }
        } else if referenceHistory.last.map({ frame.timestamp - $0.timestamp >= 0.10 }) ?? true {
            referenceHistory.append(frame)
            if referenceHistory.count > 3 { referenceHistory.removeFirst() }
        }
        guard poses.last.map({ frame.timestamp - $0.time >= 0.12 }) ?? true else { return }
        poses.append(SpatialEvidenceCollector.pose(frame))
        poses.removeAll { $0.time < frame.timestamp - (preSeconds + postSeconds + 2) }
    }

    func begin(_ frame: ARFrame) -> AmbientCaptureWindow {
        pending = true
        scanner.begin()
        let now = CACurrentMediaTime()
        let shutterDate = Date()
        let synchronized = frame.timestamp.isFinite && frame.timestamp <= now + 0.1 && now - frame.timestamp < 1
        let center = synchronized ? frame.timestamp : now
        referenceCenter = frame.timestamp.isFinite ? frame.timestamp : nil
        referenceBefore = referenceHistory.first { (0.06...0.38).contains(frame.timestamp - $0.timestamp) }
        referenceAfter = nil
        referenceHistory.removeAll()
        let date = shutterDate.addingTimeInterval(center - now)
        let remaining: Double = Double(challenge?.expiresAtMilliseconds ?? 0) / 1_000 - shutterDate.timeIntervalSince1970
        let selected = remaining > postSeconds + 12 ? challenge : nil
        return AmbientCaptureWindow(center: center, date: date, clock: synchronized ? "ARKit frame and Core Animation host uptime" : "shutter uptime fallback",
                                    sessionID: sessionID, challenge: selected, preSeconds: preSeconds,
                                    postSeconds: postSeconds, shutterUptime: now)
    }

    func finish(_ window: AmbientCaptureWindow) async -> CompletedCaptureWindow {
        let wait = max(0, window.end - CACurrentMediaTime())
        try? await Task.sleep(for: .seconds(wait))
        let radio = scanner.finish(start: window.start, end: window.end)
        let sensors = inertial.finish(center: window.center, start: window.start, end: window.end)
        let references = [referenceBefore, referenceAfter].compactMap { $0 }
        referenceCenter = nil
        referenceBefore = nil
        referenceAfter = nil
        let motion = poses.filter { (window.start...window.end).contains($0.time) }
        pending = false
        poses.removeAll()
        challenge = nil
        challengeTask?.cancel()
        challengeTask = nil
        sessionID = UUID().uuidString.lowercased()
        prepareChallenge()
        let start = window.date.addingTimeInterval(-window.preSeconds)
        let end = window.date.addingTimeInterval(window.postSeconds)
        let candidates = await trustedAnchors.observations(from: start, through: end)
        var seen: [String: Set<Data>] = [:]
        let anchors = candidates.compactMap { item -> TrustedAnchorEvidence? in
            let value = item.evidence
            guard (start...end).contains(item.observedAt), let key = trustedKeys[value.issuer],
                  SignedAnchorValidator.verify(value, pinnedKey: key, observedAt: item.observedAt),
                  seen[value.issuer, default: []].insert(value.token).inserted else { return nil }
            return value
        }
        return CompletedCaptureWindow(window: window, radio: radio, poses: motion, trustedAnchors: anchors,
                                      inertial: sensors, cameraReferences: references)
    }

    func submit(root: String, window: AmbientCaptureWindow) async -> AnchorReceipt? {
        guard let challenge = window.challenge, let attestation else { return nil }
        return try? await attestation.submit(root: root, sessionID: window.sessionID, challenge: challenge)
    }

    private func prepareChallenge() {
        guard active, !pending, challengeTask == nil, let attestation else { return }
        challengeTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let id = self?.sessionID else { return }
                let offer = try? await attestation.challenge(sessionID: id)
                guard !Task.isCancelled else { return }
                if self?.sessionID == id, self?.pending == false { self?.challenge = offer }
                let delay = offer.map { max(2, Double($0.expiresAtMilliseconds) / 1000 - Date().timeIntervalSince1970 - 25) } ?? 10
                try? await Task.sleep(for: .seconds(delay))
            }
        }
    }
}
