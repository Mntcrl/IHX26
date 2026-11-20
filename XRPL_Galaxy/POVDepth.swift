import Foundation
import Security

/// LiDAR depth → masked inverse-depth samples → Shamir shares over F_P (plan C3).
/// Must match `pov/src/planarity.rs` bit for bit.
nonisolated enum POVDepth {
    static let depthWidth = 256
    static let depthHeight = 192
    static let gridWidth = 64
    static let gridHeight = 48
    static let k = 3072
    static let fdepthBytes = 16 + 256 * 192 * 4 // 196,620
    static let confBytes = 256 * 192            // 49,152
    static let shareBytesPerNode = 3 * 3072 * 8 // 73,728
    static let p: UInt64 = (1 << 61) - 1

    /// Reads the `.fdepth` written by `saveCapture` and the confidence map from the sibling `.json`
    /// (`depth.confidence.data`, base64 UInt8 256×192).
    static func load(fdepthURL: URL, jsonURL: URL) throws -> (fdepth: Data, depth: [Float32], conf: [UInt8]) {
        let fdepth = try Data(contentsOf: fdepthURL)
        guard fdepth.count == fdepthBytes else { throw POVError.badScan("depth file is \(fdepth.count) B") }
        let bytes = [UInt8](fdepth)
        guard bytes[0..<4].elementsEqual("FLDP".utf8) else { throw POVError.badScan("depth magic") }
        func u32(_ at: Int) -> UInt32 {
            UInt32(bytes[at]) | UInt32(bytes[at + 1]) << 8 | UInt32(bytes[at + 2]) << 16 | UInt32(bytes[at + 3]) << 24
        }
        guard u32(4) == UInt32(depthWidth), u32(8) == UInt32(depthHeight) else { throw POVError.badScan("depth size") }
        var depth = [Float32](repeating: 0, count: depthWidth * depthHeight)
        for i in 0..<depth.count {
            depth[i] = Float32(bitPattern: u32(16 + 4 * i))
        }

        let json = try JSONSerialization.jsonObject(with: Data(contentsOf: jsonURL))
        guard let root = json as? [String: Any],
              let depthInfo = root["depth"] as? [String: Any],
              let confidence = depthInfo["confidence"] as? [String: Any],
              let encoded = confidence["data"] as? String,
              let conf = Data(base64Encoded: encoded) else {
            throw POVError.badScan("no depth confidence in scan JSON")
        }
        guard conf.count == confBytes else { throw POVError.badScan("confidence is \(conf.count) B") }
        return (fdepth, depth, [UInt8](conf))
    }

    /// C3.2–3: centre window, stride 2. M = high-confidence mask, W = round(65536 / z), Q = W².
    static func encode(depth: [Float32], conf: [UInt8]) -> (m: [UInt64], w: [UInt64], q: [UInt64]) {
        var m = [UInt64](repeating: 0, count: k)
        var w = [UInt64](repeating: 0, count: k)
        var q = [UInt64](repeating: 0, count: k)
        for v in 0..<gridHeight {
            for u in 0..<gridWidth {
                let x = 64 + 2 * u, y = 48 + 2 * v
                let index = v * gridWidth + u
                let z: Float32 = depth[y * depthWidth + x]
                let c = conf[y * depthWidth + x]
                guard c == 2, z.isFinite, z >= Float32(0.1), z <= Float32(10.0) else { continue }
                let weight = UInt64((65536.0 / Double(z)).rounded())
                m[index] = 1
                w[index] = weight
                q[index] = weight * weight
            }
        }
        return (m, w, q)
    }

    /// C3.4: degree-1 Shamir over P = 2^61 − 1; result[i] = shares for node i + 1.
    static func split3(_ secrets: [UInt64]) -> [[UInt64]] {
        var random = [UInt64](repeating: 0, count: secrets.count)
        let status = random.withUnsafeMutableBytes { SecRandomCopyBytes(kSecRandomDefault, $0.count, $0.baseAddress!) }
        precondition(status == errSecSuccess, "SecRandomCopyBytes failed")
        var shares = [[UInt64]](repeating: [UInt64](repeating: 0, count: secrets.count), count: 3)
        for (j, secret) in secrets.enumerated() {
            var a = random[j] & p
            if a == p { a = 0 }
            for i in 0..<3 {
                shares[i][j] = (secret + a * UInt64(i + 1)) % p
            }
        }
        return shares
    }

    /// C3.5: share bytes for node i = M_i ‖ W_i ‖ Q_i, each 3072 × u64 little-endian; index 0 = node 1.
    static func shareBytes(m: [UInt64], w: [UInt64], q: [UInt64]) -> [Data] {
        let sm = split3(m), sw = split3(w), sq = split3(q)
        return (0..<3).map { i in
            var out = Data(capacity: shareBytesPerNode)
            for vector in [sm[i], sw[i], sq[i]] {
                for value in vector {
                    withUnsafeBytes(of: value.littleEndian) { out.append(contentsOf: $0) }
                }
            }
            return out
        }
    }
}
