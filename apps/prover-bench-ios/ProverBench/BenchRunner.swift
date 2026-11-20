import Foundation
import SwiftUI
import UIKit
import galaxy_proverFFI

/// Benchmark artifacts for one circuit: <dir>/<name>.graph.bin, <name>.zkey, input.json.
struct BenchCircuit {
    let name: String
    let dir: URL
    var graph: URL { dir.appendingPathComponent("\(name).graph.bin") }
    var zkey: URL { dir.appendingPathComponent("\(name).zkey") }
    var input: URL { dir.appendingPathComponent("input.json") }
    var zkeyMB: Double { fileMB(zkey) }
    var graphMB: Double { fileMB(graph) }
}

func fileMB(_ url: URL) -> Double {
    let size = (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? NSNumber)?.doubleValue ?? 0
    return size / 1_048_576
}

/// phys_footprint (what jetsam enforces) and the kernel's lifetime peak, bytes.
func memoryFootprint() -> (current: UInt64, lifetimePeak: UInt64) {
    var info = task_vm_info_data_t()
    var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
    let kr = withUnsafeMutablePointer(to: &info) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
            task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
        }
    }
    guard kr == KERN_SUCCESS else { return (0, 0) }
    return (info.phys_footprint, UInt64(clamping: info.ledger_phys_footprint_peak))
}

func osVersion() -> String {
    let v = ProcessInfo.processInfo.operatingSystemVersion
    return "\(v.majorVersion).\(v.minorVersion).\(v.patchVersion)"
}

/// Samples phys_footprint every 10 ms and keeps the maximum.
final class FootprintSampler {
    private let lock = NSLock()
    private var peak: UInt64 = 0
    private var timer: DispatchSourceTimer?

    func start() {
        lock.lock(); peak = memoryFootprint().current; lock.unlock()
        let t = DispatchSource.makeTimerSource(queue: DispatchQueue.global(qos: .utility))
        t.schedule(deadline: .now(), repeating: .milliseconds(10))
        t.setEventHandler { [weak self] in
            guard let self else { return }
            let now = memoryFootprint().current
            self.lock.lock(); if now > self.peak { self.peak = now }; self.lock.unlock()
        }
        t.resume()
        timer = t
    }

    func stop() -> UInt64 {
        timer?.cancel()
        timer = nil
        lock.lock(); defer { lock.unlock() }
        return max(peak, memoryFootprint().current)
    }
}

func thermalName(_ s: ProcessInfo.ThermalState) -> String {
    switch s {
    case .nominal: return "nominal"
    case .fair: return "fair"
    case .serious: return "serious"
    case .critical: return "critical"
    @unknown default: return "unknown"
    }
}

func deviceModel() -> String {
    var u = utsname()
    uname(&u)
    return withUnsafeBytes(of: &u.machine) { raw in
        String(decoding: raw.prefix(while: { $0 != 0 }), as: UTF8.self)
    }
}

final class PhaseLogger: ProgressListener {
    let emit: @Sendable (String) -> Void
    init(emit: @escaping @Sendable (String) -> Void) { self.emit = emit }
    func onPhaseStarted(phase: String) { emit("  > \(phase)") }
    func onPhaseFinished(phase: String, seconds: Double) {
        emit(String(format: "  < %@ %.3f s (footprint %.0f MB)", phase, seconds,
                    Double(memoryFootprint().current) / 1_048_576))
    }
}

@MainActor
final class BenchRunner: ObservableObject {
    @Published var circuits: [BenchCircuit] = []
    @Published var status = "idle"
    @Published var log: [String] = []
    @Published var busy = false
    private var cancelToken: CancelToken?

    /// The only directory this app ever writes to or deletes (spec §19 device data safety).
    /// Under the fallback bundle id the container is shared with the capture app, whose
    /// files elsewhere in Documents must never be touched.
    nonisolated static let rootName = "galaxy-proverbench"
    private let root = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        .appendingPathComponent(BenchRunner.rootName, isDirectory: true)

    init() { scan() }

    func scan() {
        var found: [BenchCircuit] = []
        var roots = [root]
        if let res = Bundle.main.resourceURL { roots.append(res.appendingPathComponent("artifacts")) }
        for dir in roots {
            let dirs = (try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil)) ?? []
            for d in dirs.sorted(by: { $0.lastPathComponent < $1.lastPathComponent }) {
                let c = BenchCircuit(name: d.lastPathComponent, dir: d)
                let fm = FileManager.default
                if fm.fileExists(atPath: c.graph.path), fm.fileExists(atPath: c.zkey.path),
                   fm.fileExists(atPath: c.input.path), !found.contains(where: { $0.name == c.name }) {
                    found.append(c)
                }
            }
        }
        circuits = found
    }

    /// Launch arguments: `--autorun name1,name2 [--repeat N] [--msm-chunk N] [--exit]`, or
    /// `--cleanup`: logs every file under `Documents/galaxy-proverbench/` and deletes exactly
    /// that directory (nothing else in Documents is listed or touched).
    func autorunIfRequested() {
        let args = ProcessInfo.processInfo.arguments
        if args.contains("--cleanup") {
            let fm = FileManager.default
            guard fm.fileExists(atPath: root.path) else {
                emit("CLEANUP nothing to remove at Documents/\(BenchRunner.rootName)")
                exit(0)
            }
            let walker = fm.enumerator(at: root, includingPropertiesForKeys: [.fileSizeKey])
            while let f = walker?.nextObject() as? URL {
                let size = (try? f.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0
                emit("CLEANUP target \(f.path.replacingOccurrences(of: root.path, with: "Documents/\(BenchRunner.rootName)")) \(size)")
            }
            do {
                try fm.removeItem(at: root)
                emit("CLEANUP removed Documents/\(BenchRunner.rootName)")
                exit(0)
            } catch {
                emit("ERROR remove Documents/\(BenchRunner.rootName): \(error)")
                exit(1)
            }
        }
        func value(_ k: String) -> String? {
            guard let i = args.firstIndex(of: k), i + 1 < args.count else { return nil }
            return args[i + 1]
        }
        guard let names = value("--autorun") else { return }
        let repeats = Int(value("--repeat") ?? "1") ?? 1
        let chunk = UInt32(value("--msm-chunk") ?? "0") ?? 0
        run(names: names.split(separator: ",").map(String.init), repeats: repeats, msmChunk: chunk,
            exitWhenDone: args.contains("--exit"))
    }

    func cancel() { cancelToken?.cancel() }

    private func emit(_ line: String) {
        print("GXBENCH \(line)")
        fflush(stdout)
        log.append(line)
    }

    func run(names: [String], repeats: Int, msmChunk: UInt32 = 0, exitWhenDone: Bool = false) {
        let selected = names.compactMap { n in circuits.first(where: { $0.name == n }) }
        if selected.count != names.count {
            emit("ERROR unknown circuit in \(names); available: \(circuits.map(\.name))")
            if exitWhenDone { exit(2) }
            return
        }
        busy = true
        let token = CancelToken()
        cancelToken = token
        let docs = root
        let emitter: @Sendable (String) -> Void = { line in Task { @MainActor in self.emit(line) } }
        Thread.detachNewThread { [weak self] in
            var failures = 0
            for c in selected {
                for i in 0..<repeats {
                    Task { @MainActor in self?.status = "running \(c.name) #\(i)" }
                    do {
                        try BenchRunner.runOne(c, iteration: i, msmChunk: msmChunk, cancel: token,
                                               root: docs, emit: emitter)
                    } catch {
                        failures += 1
                        emitter("ERROR \(c.name) #\(i): \(error)")
                    }
                }
            }
            Task { @MainActor in
                guard let self else { return }
                self.busy = false
                self.status = failures == 0 ? "done" : "done with \(failures) failure(s)"
                self.emit("DONE failures=\(failures)")
                if exitWhenDone {
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1) { exit(failures == 0 ? 0 : 1) }
                }
            }
        }
    }

    nonisolated static func runOne(_ c: BenchCircuit, iteration: Int, msmChunk: UInt32, cancel: CancelToken,
                                   root: URL, emit: @escaping @Sendable (String) -> Void) throws {
        let thermalBefore = ProcessInfo.processInfo.thermalState
        let before = memoryFootprint()
        let availableBefore = os_proc_available_memory()
        emit("RUN \(c.name) #\(iteration) model=\(deviceModel()) ios=\(osVersion()) "
             + "cpus=\(ProcessInfo.processInfo.activeProcessorCount) thermal=\(thermalName(thermalBefore)) "
             + String(format: "footprint=%.0fMB available=%.0fMB", Double(before.current) / 1_048_576,
                      Double(availableBefore) / 1_048_576))
        let sampler = FootprintSampler()
        sampler.start()
        let wall0 = Date()
        let t0 = Date()
        let prover = try GalaxyProver.open(graphPath: c.graph.path, zkeyPath: c.zkey.path, msmChunk: msmChunk)
        let openSeconds = Date().timeIntervalSince(t0)
        let input = try String(contentsOf: c.input, encoding: .utf8)
        let result = try prover.prove(inputsJson: input, listener: PhaseLogger(emit: emit), cancel: cancel)
        let wall = Date().timeIntervalSince(wall0)
        let peak = sampler.stop()
        let after = memoryFootprint()
        let thermalAfter = ProcessInfo.processInfo.thermalState
        // Independent verification from the wire encodings (fresh decode of VK, inputs, proof).
        let vk = try prover.vkBytes()
        let verified = try verifyProof(vkBytes: vk, publicInputs: result.publicInputs, proof: result.proof)

        let phases = Dictionary(uniqueKeysWithValues: result.phases.map { ($0.phase, $0.seconds) })
        let stamp = ISO8601DateFormatter().string(from: Date()).replacingOccurrences(of: ":", with: "")
        let summary: [String: Any] = [
            "circuit": c.name,
            "iteration": iteration,
            "device_model": deviceModel(),
            "ios_version": osVersion(),
            "active_cpus": ProcessInfo.processInfo.activeProcessorCount,
            "n_vars": prover.nVars(),
            "domain_size": prover.domainSize(),
            "n_public": prover.nPublic(),
            "msm_chunk": msmChunk,
            "zkey_mb": c.zkeyMB,
            "graph_mb": c.graphMB,
            "open_seconds": openSeconds,
            "witness_seconds": result.witnessSeconds,
            "prove_seconds": result.proveSeconds,
            "wall_seconds": wall,
            "phases": phases,
            "footprint_before_mb": Double(before.current) / 1_048_576,
            "footprint_peak_sampled_mb": Double(peak) / 1_048_576,
            "footprint_lifetime_peak_mb": Double(after.lifetimePeak) / 1_048_576,
            "available_before_mb": Double(availableBefore) / 1_048_576,
            "thermal_before": thermalName(thermalBefore),
            "thermal_after": thermalName(thermalAfter),
            "verified_independently": verified,
            "proof_hex": result.proof.map { String(format: "%02x", $0) }.joined(),
        ]
        let outDir = root.appendingPathComponent("results", isDirectory: true)
        try FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)
        let base = "\(c.name)-\(stamp)-\(iteration)"
        let json = try JSONSerialization.data(withJSONObject: summary, options: [.prettyPrinted, .sortedKeys])
        try json.write(to: outDir.appendingPathComponent("\(base).json"))
        try result.snarkjsProofJson.write(to: outDir.appendingPathComponent("\(base).proof.json"),
                                          atomically: true, encoding: .utf8)
        try result.snarkjsPublicJson.write(to: outDir.appendingPathComponent("\(base).public.json"),
                                           atomically: true, encoding: .utf8)
        try vk.write(to: outDir.appendingPathComponent("\(base).vk.bin"))
        emit(String(format: "RESULT %@ #%d open=%.3fs witness=%.3fs prove=%.3fs wall=%.3fs peak=%.0fMB "
                    + "lifetime_peak=%.0fMB thermal=%@->%@ verified=%@ file=%@",
                    c.name, iteration, openSeconds, result.witnessSeconds, result.proveSeconds, wall,
                    Double(peak) / 1_048_576, Double(after.lifetimePeak) / 1_048_576,
                    thermalName(thermalBefore), thermalName(thermalAfter), verified ? "true" : "false", base))
        emit("RESULTJSON \(String(decoding: try JSONSerialization.data(withJSONObject: summary.filter { $0.key != "proof_hex" }, options: [.sortedKeys]), as: UTF8.self))")
    }
}
