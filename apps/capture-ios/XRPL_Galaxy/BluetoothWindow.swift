import CoreBluetooth
import CryptoKit
import Foundation
import QuartzCore

nonisolated struct BluetoothAggregate: Encodable, Sendable {
    let version: UInt8 = 1
    var flags: UInt8 = 0
    var scan_duration_ms: UInt32 = 0
    var n_adv: UInt32 = 0
    var n_unique: UInt32 = 0
    var rssi_histogram = [UInt32](repeating: 0, count: 16)
    var rssi_mean_q8: Int32 = 0
    var rssi_var_q8: UInt32 = 0
    var persistence_pct: UInt8 = 0
}

@MainActor
final class BluetoothWindow: NSObject, @preconcurrency CBCentralManagerDelegate {
    private struct Observation {
        let identifier: HashedAuthenticationCode<SHA256>
        let time: Double
        /// Clamped to the committed range [−100, 0] dBm.
        let rssi: Int
    }

    private var manager: CBCentralManager?
    private var key: SymmetricKey?
    private var observations: [Observation] = []
    private var active = false
    private var scanStart: Double?

    var status: String {
        guard let manager else { return "Not scanning" }
        if [.denied, .restricted].contains(CBCentralManager.authorization) { return "Not authorized (optional)" }
        if manager.state == .poweredOff { return "Bluetooth off (optional)" }
        return manager.isScanning ? "Foreground scan" : "Not scanning (optional)"
    }

    func start() {
        active = true
        if manager == nil {
            manager = CBCentralManager(delegate: self, queue: .main,
                                       options: [CBCentralManagerOptionShowPowerAlertKey: false])
        }
        updateScan()
    }

    func stop() {
        active = false
        updateScan()
    }

    private func updateScan() {
        guard let manager else { return }
        if active && manager.state == .poweredOn && CBCentralManager.authorization == .allowedAlways {
            if !manager.isScanning {
                key = SymmetricKey(size: .bits256)
                scanStart = CACurrentMediaTime()
                manager.scanForPeripherals(withServices: nil,
                                           options: [CBCentralManagerScanOptionAllowDuplicatesKey: true])
            }
        } else {
            manager.stopScan()
            scanStart = nil
            key = nil
            observations.removeAll(keepingCapacity: true)
        }
    }

    func centralManagerDidUpdateState(_ central: CBCentralManager) { updateScan() }

    func centralManager(_ central: CBCentralManager, didDiscover peripheral: CBPeripheral,
                        advertisementData: [String: Any], rssi RSSI: NSNumber) {
        guard active, central.isScanning, let key else { return }
        let now = CACurrentMediaTime()
        if let first = observations.first, first.time < now - 6 {
            let obsolete = observations.firstIndex { $0.time >= now - 6 } ?? observations.count
            observations.removeFirst(obsolete)
        }
        // Observations are contextual only. Drop excess advertisements rather than growing unbounded.
        guard observations.count < 12_000 else { return }
        var uuid = peripheral.identifier.uuid
        let identifier = withUnsafeBytes(of: &uuid) { HMAC<SHA256>.authenticationCode(for: Data($0), using: key) }
        // 127 dBm is CoreBluetooth's "unavailable" value; such reports carry no signal statistic.
        let rssi = RSSI.intValue
        guard (-127...20).contains(rssi) else { return }
        observations.append(Observation(identifier: identifier, time: now, rssi: min(0, max(-100, rssi))))
    }

    func freeze(center: Double, until end: Double) -> BluetoothAggregate {
        defer {
            observations.removeAll(keepingCapacity: true)
            key = manager?.isScanning == true ? SymmetricKey(size: .bits256) : nil
            scanStart = key == nil ? nil : CACurrentMediaTime()
        }
        var result = BluetoothAggregate()
        if manager?.state == .poweredOn { result.flags |= 1 }
        if CBCentralManager.authorization == .allowedAlways { result.flags |= 2 }
        guard result.flags == 3, manager?.isScanning == true, let scanStart else { return result }
        let start = max(scanStart, center - 5)
        guard end > start else { return result }
        result.flags |= 4
        result.scan_duration_ms = UInt32(((end - start) * 1_000).rounded())
        var seen: [HashedAuthenticationCode<SHA256>: UInt8] = [:]
        var n = 0.0, sum = 0.0, squares = 0.0
        let middle = (start + end) / 2
        for observation in observations where (start...end).contains(observation.time) {
            // n_adv equals the histogram total, as the evidence validator requires.
            result.n_adv += 1
            seen[observation.identifier, default: 0] |= observation.time < middle ? 1 : 2
            let rssi = observation.rssi
            result.rssi_histogram[min(15, (rssi + 100) / 5)] += 1
            n += 1
            sum += Double(rssi)
            squares += Double(rssi * rssi)
        }
        result.n_unique = UInt32(seen.count)
        if !seen.isEmpty {
            let persistent = seen.values.reduce(0) { $0 + ($1 == 3 ? 1 : 0) }
            result.persistence_pct = UInt8((100 * persistent + seen.count / 2) / seen.count)
        }
        if n > 0 {
            let mean = sum / n
            result.rssi_mean_q8 = Int32((256 * mean).rounded())
            result.rssi_var_q8 = UInt32((256 * max(0, squares / n - mean * mean)).rounded())
        }
        return result
    }
}
