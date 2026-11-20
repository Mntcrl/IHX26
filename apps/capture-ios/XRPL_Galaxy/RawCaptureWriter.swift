import Foundation
import simd
import UIKit

nonisolated struct NoiseAggregate: Encodable, Sendable {
    let version: UInt8 = 1
    let stats: [Int16]

    init(luma: Data) {
        // Four quadrants, each with mean absolute residual, RMS, signed mean (Q6), and
        // percentage of |residual| > 8 (Q6). Sample a fixed sparse grid; never form a residual image.
        stats = luma.withUnsafeBytes { (pixels: UnsafeRawBufferPointer) in
            let width = FrameSnapshot.width, height = FrameSnapshot.height
            var stats = [Int16]()
            stats.reserveCapacity(16)
            for q in 0..<4 {
                let x0 = (q % 2) * (width / 2), y0 = (q / 2) * (height / 2)
                var sum = 0, squares = 0, absolute = 0, large = 0, count = 0
                for y in stride(from: y0 + 1, to: y0 + height / 2 - 1, by: 8) {
                    for x in stride(from: x0 + 1, to: x0 + width / 2 - 1, by: 8) {
                        let i = y * width + x
                        let neighbor = Int(pixels[i - 1]) + Int(pixels[i + 1])
                                     + Int(pixels[i - width]) + Int(pixels[i + width])
                        let r = Int(pixels[i]) - (neighbor + 2) / 4
                        sum += r
                        squares += r * r
                        absolute += abs(r)
                        large += abs(r) > 8 ? 1 : 0
                        count += 1
                    }
                }
                let n = Double(count)
                stats += [Double(absolute) / n, sqrt(Double(squares) / n), Double(sum) / n, 100 * Double(large) / n]
                    .map { Int16((64 * $0).rounded()) }
            }
            return stats
        }
    }
}

nonisolated enum RawCaptureWriter {
    struct Device: Encodable, Sendable {
        let model: String
        let os: String
        let app_build: String

        @MainActor static func current() -> Device {
            var system = utsname()
            uname(&system)
            let model = withUnsafeBytes(of: &system.machine) { String(decoding: $0.prefix { $0 != 0 }, as: UTF8.self) }
            return Device(model: model, os: UIDevice.current.systemVersion,
                          app_build: Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "0")
        }
    }

    enum Failure: LocalizedError {
        case invalidSnapshot, headerTooLarge
        var errorDescription: String? {
            switch self {
            case .invalidSnapshot: "The snapshot does not satisfy the GXRW v1 capture format."
            case .headerTooLarge: "The GXRW v1 capture header exceeded 1 MiB."
            }
        }
    }

    private struct Region: Encodable { let offset: UInt64; let len: UInt64 }
    private struct Image: Encodable {
        let width = FrameSnapshot.width
        let height = FrameSnapshot.height
        let pixel_format = "420f"
        let ycbcr_matrix: String
    }
    private struct Depth: Encodable { let width = FrameSnapshot.depthWidth; let height = FrameSnapshot.depthHeight }
    private struct Frame: Encodable {
        let role: String
        let timestamp_s: Double
        let intrinsics: [Double]
        let transform: [Double]
        let tracking: UInt8
        let tracking_reason: String
        let y: Region
        let cbcr: Region
        let depth: Region
        let conf: Region
    }
    private struct IMU: Encodable { let samples: [[Double]] }
    private struct Quality: Encodable { let flags: UInt32; let reasons: [String] }
    private struct Header: Encodable {
        let format = "galaxy-raw-capture/v1"
        let device: Device
        let image: Image
        let depth = Depth()
        let frames: [Frame]
        let imu: IMU
        let bluetooth: BluetoothAggregate
        let noise: NoiseAggregate
        let claimed_time_ms: UInt64
        let quality: Quality
    }

    static func write(frames: [FrameSnapshot], motion: [MotionSample], bluetooth: BluetoothAggregate,
                      device: Device, record: CaptureRecord, issues: [CaptureIssue], to destination: URL) throws {
        guard frames.count == 3, let center = frames[1].pixels,
              motion.first.map({ $0.timestamp <= frames[0].timestamp - 0.05 }) == true,
              motion.last.map({ $0.timestamp >= frames[2].timestamp + 0.05 }) == true else { throw Failure.invalidSnapshot }
        var offset: UInt64 = 0
        var payload: [Data] = []
        payload.reserveCapacity(12)
        func region(_ data: Data) -> Region {
            let count = UInt64(data.count)
            let result = Region(offset: offset, len: count)
            payload.append(data)
            offset += count
            return result
        }
        let encodedFrames = try zip(["minus", "center", "plus"], frames).map { role, frame in
            guard let pixels = frame.pixels, let depth = frame.depth else { throw Failure.invalidSnapshot }
            let m = frame.intrinsics.columns
            let intrinsics = [m.0.x, m.1.y, m.2.x, m.2.y].map(Double.init)
            let t = frame.transform.columns
            let transform = [t.0, t.1, t.2, t.3].flatMap { [Double($0.x), Double($0.y), Double($0.z), Double($0.w)] }
            return Frame(role: role, timestamp_s: frame.timestamp, intrinsics: intrinsics, transform: transform,
                         tracking: frame.tracking, tracking_reason: frame.trackingReason,
                         y: region(pixels.y), cbcr: region(pixels.cbcr), depth: region(depth.meters), conf: region(depth.confidence))
        }
        let header = Header(device: device, image: Image(ycbcr_matrix: center.matrix), frames: encodedFrames,
                            imu: IMU(samples: motion.map(\.encoded)), bluetooth: bluetooth,
                            noise: NoiseAggregate(luma: center.y), claimed_time_ms: record.claimedTimeMs,
                            quality: Quality(flags: issues.reduce(0) { $0 | $1.rawValue }, reasons: issues.map(\.message)))
        let data = try JSONEncoder().encode(header)
        guard data.count <= 1 << 20 else { throw Failure.headerTooLarge }
        var prefix = Data([0x47, 0x58, 0x52, 0x57, 0x01])
        var length = UInt32(data.count).bigEndian
        withUnsafeBytes(of: &length) { prefix.append(contentsOf: $0) }
        // The capture directory is new for every capture, so the in-progress path cannot exist yet;
        // `create` refuses it otherwise. Only that file, created here, is removed on failure.
        let temp = destination.deletingLastPathComponent().appending(path: ".capture.gxrw.in-progress")
        try ProtectedFiles.create(prefix, at: temp)
        do {
            let handle = try FileHandle(forWritingTo: temp)
            defer { try? handle.close() }
            try handle.seekToEnd()
            try handle.write(contentsOf: data)
            for plane in payload { try handle.write(contentsOf: plane) }
            try handle.synchronize()
            // Atomic publication (fails if the destination exists); recovery never sees a partial capture.
            try FileManager.default.moveItem(at: temp, to: destination)
        } catch {
            try? FileManager.default.removeItem(at: temp)
            throw error
        }
    }
}
