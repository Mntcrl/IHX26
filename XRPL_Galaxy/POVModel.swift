import Foundation
import Combine
import SwiftUI

/// App-side state of the POV committee path: coordinator config, node-key check against XRPL,
/// T0 prefetch, capture submission and publishing.
@MainActor
final class POVModel: ObservableObject {
    static let coordinatorDefaultsKey = "pov.coordinatorURL"
    static let defaultCoordinatorURL = "http://10.164.228.104:8088"
    static let defaultRPC = "https://s.altnet.rippletest.net:51234/"
    /// A prefetched T0 older than this at the shutter is not used (C2).
    static let maxT0AgeSeconds: TimeInterval = 20
    static let t0RefreshSeconds: Double = 3

    @Published var coordinatorURL: String
    @Published var config: ConfigResponse?
    @Published var keysOK: Bool?
    @Published var step = "Connecting to committee…"
    @Published var busy = false
    @Published var result: Attempt?
    @Published var resultJPEG: URL?
    @Published var published: String?
    @Published var publishing = false
    @Published var showSettings = false
    @Published var devicePubkey = ""

    private var latestT0: (T0, Date)?
    private var refreshLoop: Task<Void, Never>?

    init() {
        if let saved = UserDefaults.standard.string(forKey: Self.coordinatorDefaultsKey), !saved.isEmpty {
            coordinatorURL = saved
        } else if let bundled = Bundle.main.object(forInfoDictionaryKey: "POVCoordinatorURL") as? String,
                  !bundled.trimmingCharacters(in: .whitespaces).isEmpty {
            coordinatorURL = bundled
        } else {
            coordinatorURL = Self.defaultCoordinatorURL
        }
        do {
            devicePubkey = POVCrypto.pubkeyHex(try POVCrypto.deviceKey())
        } catch {
            step = "Device key: \(error.localizedDescription)"
        }
    }

    /// Loads the committee config, checks the node keys on XRPL, then refreshes T0 every 3 s
    /// (and retries the config until it succeeds). Calling it again restarts everything.
    func start() async {
        refreshLoop?.cancel()
        config = nil
        keysOK = nil
        latestT0 = nil
        await loadConfig()
        refreshLoop = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                await self.refresh()
                try? await Task.sleep(for: .seconds(Self.t0RefreshSeconds))
            }
        }
    }

    /// The prefetched validated ledger, if fetched at most 20 s ago. Read it before the shutter frame.
    func freshT0() -> T0? {
        guard let latest = latestT0, Date().timeIntervalSince(latest.1) <= Self.maxT0AgeSeconds else { return nil }
        return latest.0
    }

    func saveSettings(coordinatorURL url: String) {
        let trimmed = url.trimmingCharacters(in: .whitespacesAndNewlines)
        UserDefaults.standard.set(trimmed, forKey: Self.coordinatorDefaultsKey)
        coordinatorURL = trimmed
        Task { await start() }
    }

    /// Hashes, signs (Secure Enclave), Shamir-splits and HPKE-seals a saved capture, then waits
    /// for the committee's verdict. Presents the result sheet.
    func submit(jpegURL: URL, fdepthURL: URL, jsonURL: URL, evidenceRoot: String, t0: T0) async {
        guard !busy else { step = "Previous capture still verifying"; return }
        guard let config, let base = URL(string: coordinatorURL) else {
            step = "Verification offline: no committee config"
            return
        }
        guard keysOK == true else {
            step = "Verification offline: node keys not confirmed on XRPL"
            return
        }
        busy = true
        defer { busy = false }
        step = "Signing and sealing depth shares…"
        do {
            let request = try await Task.detached(priority: .userInitiated) {
                try Self.buildRequest(jpegURL: jpegURL, fdepthURL: fdepthURL, jsonURL: jsonURL,
                                      evidenceRoot: evidenceRoot, t0: t0, config: config)
            }.value
            step = "Committee verifying…"
            let attempt = try await POVClient(base: base).capture(request)
            published = attempt.published
            resultJPEG = jpegURL
            result = attempt
            step = attempt.passed ? "Anchored on XRPL" : "Rejected: \(attempt.reason ?? "unknown")"
        } catch {
            step = "Verification failed: \(error.localizedDescription)"
        }
    }

    /// Uploads only the JPEG of the shown PASS result (never the sensor-noise files).
    func publish() async {
        guard let result, result.passed, let jpegURL = resultJPEG, let base = URL(string: coordinatorURL) else { return }
        publishing = true
        defer { publishing = false }
        do {
            let jpeg = try await Task.detached(priority: .userInitiated) { try Data(contentsOf: jpegURL) }.value
            published = try await POVClient(base: base).publish(captureId: result.capture_id, jpeg: jpeg)
        } catch {
            step = "Publish failed: \(error.localizedDescription)"
        }
    }

    private func refresh() async {
        if config == nil || keysOK == nil { await loadConfig() }
        let rpc = URL(string: config?.xrpl_rpc ?? Self.defaultRPC) ?? URL(string: Self.defaultRPC)!
        if let t0 = try? await POVClient.fetchT0(rpc: rpc) {
            latestT0 = (t0, Date())
        }
    }

    private func loadConfig() async {
        guard let base = URL(string: coordinatorURL), base.scheme != nil, base.host != nil else {
            note("Invalid coordinator URL")
            return
        }
        do {
            let loaded = try await POVClient(base: base).config()
            config = loaded
            note("Checking node keys on XRPL…")
            let match = try await POVClient.nodeKeysMatch(loaded)
            keysOK = match
            note(match ? "Committee ready" : "Node keys do not match their XRPL DIDs")
        } catch {
            note("Committee: \(error.localizedDescription)")
        }
    }

    /// Background status updates never overwrite the progress of a running submission.
    private func note(_ message: String) {
        if !busy { step = message }
    }

    nonisolated static func buildRequest(jpegURL: URL, fdepthURL: URL, jsonURL: URL, evidenceRoot: String,
                                         t0: T0, config: ConfigResponse) throws -> CaptureRequest {
        let jpeg = try Data(contentsOf: jpegURL)
        let scan = try POVDepth.load(fdepthURL: fdepthURL, jsonURL: jsonURL)
        let encoded = POVDepth.encode(depth: scan.depth, conf: scan.conf)
        let shares = POVDepth.shareBytes(m: encoded.m, w: encoded.w, q: encoded.q)
        let key = try POVCrypto.deviceKey()
        let manifest = Manifest(
            v: 1,
            app: "pov-ios/1",
            committee: config.committee_account,
            t0: t0,
            captured_at: ISO8601DateFormatter().string(from: Date()),
            image_sha256: POVCrypto.sha256Hex(jpeg),
            depth_sha256: POVCrypto.sha256Hex(scan.fdepth),
            evidence_root: evidenceRoot.lowercased(),
            grid: [POVDepth.gridWidth, POVDepth.gridHeight],
            shares_sha256: shares.map(POVCrypto.sha256Hex),
            device_pubkey: POVCrypto.pubkeyHex(key)
        )
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        let manifestBytes = try encoder.encode(manifest)
        let signature = try POVCrypto.sign(manifestBytes, key: key)
        let aad = POVCrypto.sha256(manifestBytes)
        var sealed: [EncShare] = []
        for id in 1...3 {
            guard let node = config.nodes.first(where: { $0.id == id }) else {
                throw POVError.notReady("committee node \(id) missing from config")
            }
            let box = try POVCrypto.sealShare(shares[id - 1], hpkePkHex: node.hpke_pk, aad: aad)
            sealed.append(EncShare(node_id: id, enc_b64: box.enc.base64EncodedString(), ct_b64: box.ct.base64EncodedString()))
        }
        return CaptureRequest(manifest_b64: manifestBytes.base64EncodedString(), signature_hex: signature, shares: sealed)
    }
}
