import SwiftUI

nonisolated struct CaptureProofSummary: Sendable {
    var proof: EvidenceCommitment?
    var integrity = false
    var radio = "Unavailable"
    var spatial = "Unavailable"
    var anchors = false
    var inertial: String?
    var cameraNoise: String?

    static func load(proofURL: URL, radioURL: URL, spatialURL: URL, imuURL: URL, noiseURL: URL) -> Self {
        var report = Self()
        let decoder = JSONDecoder()
        guard let bytes = try? Data(contentsOf: proofURL),
              let proof = try? decoder.decode(EvidenceCommitment.self, from: bytes) else { return report }
        report.proof = proof
        report.integrity = EvidenceVerifier.verifiesAll(proof, folder: proofURL.deletingLastPathComponent())
        guard report.integrity else { return report }
        if proof.disclosures.contains(where: { $0.category == "radio" && $0.fileHashes[radioURL.lastPathComponent] != nil }),
           let bytes = try? Data(contentsOf: radioURL), let radio = try? decoder.decode(RadioFingerprint.self, from: bytes) {
            if radio.observationCallbacks > 0 {
                report.radio = "\(radio.observationCallbacks) callbacks · \(radio.complete ? "full window" : "partial window")"
            } else {
                report.radio = radio.scanDurationMilliseconds > 0 ? "No callbacks observed" : "Unavailable · \(radio.availability)"
            }
        }
        if proof.disclosures.contains(where: { $0.category == "lidar" && $0.fileHashes[spatialURL.lastPathComponent] != nil }),
           let bytes = try? Data(contentsOf: spatialURL),
           let document = try? JSONSerialization.jsonObject(with: bytes) as? [String: Any],
           let checks = document["consistency"] as? [String: Any] {
            switch checks["status"] as? String {
            case "passed": report.spatial = "Checks passed"
            case "motionDiscontinuity": report.spatial = "Motion discontinuity"
            case "depthFeatureDisagreement": report.spatial = "Depth and tracking disagree"
            default: report.spatial = "Insufficient data"
            }
        }
        if let imu = committedJSON(imuURL, category: "lidar", proof: proof),
           let accelerometer = imu["accelerometer"] as? [String: Any],
           let gyro = imu["gyroscope"] as? [String: Any] {
            let accelerationCount = accelerometer["sampleCount"] as? Int ?? 0
            let gyroCount = gyro["sampleCount"] as? Int ?? 0
            report.inertial = "Accel \(accelerationCount) · Gyro \(gyroCount) samples"
        }
        if let noise = committedJSON(noiseURL, category: "photo", proof: proof) {
            let comparisons = noise["temporalComparisons"] as? [[String: Any]] ?? []
            let measured = comparisons.filter { ($0["status"] as? String) == "measured" }.count
            report.cameraNoise = measured == 0 ? "Y, Cb, Cr residual maps" : "Y, Cb, Cr maps · \(measured) time comparisons"
        }
        report.anchors = proof.disclosures.contains { $0.category == "trustedAnchors" }
        return report
    }

    private static func committedJSON(_ url: URL, category: String, proof: EvidenceCommitment) -> [String: Any]? {
        guard proof.disclosures.contains(where: { $0.category == category && $0.fileHashes[url.lastPathComponent] != nil }),
              let data = try? Data(contentsOf: url) else { return nil }
        return try? JSONSerialization.jsonObject(with: data) as? [String: Any]
    }
}

struct CaptureProofView: View {
    let summary: CaptureProofSummary?
    let receipt: AnchorReceipt?
    let networkStatus: String

    var body: some View {
        if let summary, summary.proof != nil {
            VStack(alignment: .leading, spacing: 8) {
                metric("Evidence integrity", summary.integrity ? "Verified" : "Failed")
                metric("Application attestation", receipt?.applicationAttestationVerified == true ? "Verified" : networkStatus)
                metric("Replay protection", receipt?.replayProtectionVerified == true ? "Verified" : networkStatus)
                metric("XRPL anchoring", receipt?.xrplAnchoringVerified == true ? "Verified · \(receipt?.network ?? "XRPL")" : networkStatus)
                metric("Ambient radio", summary.radio)
                metric("Spatial consistency", summary.spatial)
                if let inertial = summary.inertial { metric("Inertial samples", inertial) }
                if let noise = summary.cameraNoise { metric("Camera residuals", noise) }
                metric("Authenticated infrastructure", summary.anchors ? "Signed tokens present" : "None")
                metric("Secure physical ranging", "Unavailable")
                if let tx = receipt?.transactionHash { Text("XRPL transaction \(tx)").font(.caption2.monospaced()).textSelection(.enabled) }
                if let root = summary.proof?.root { Text("Merkle root \(root)").font(.caption2.monospaced()).textSelection(.enabled) }
            }
            .padding(14)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
        } else {
            Text(summary == nil ? "Checking evidence…" : "This scan has no ambient proof.")
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    private func metric(_ title: String, _ value: String) -> some View {
        HStack(alignment: .top) {
            Text(title)
            Spacer(minLength: 10)
            Text(value).foregroundStyle(.secondary).multilineTextAlignment(.trailing)
        }.font(.caption)
    }
}
