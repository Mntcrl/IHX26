import CoreBluetooth
import CryptoKit
import Foundation
import QuartzCore

@MainActor
final class AmbientRadioScanner: NSObject, CBCentralManagerDelegate {
    private var manager: CBCentralManager?
    private var secret = SymmetricKey(size: .bits256)
    private var observations: [RadioFingerprintBuilder.Observation] = []
    private var active = false
    private var scanningSince: Double?
    private var coverage: [ClosedRange<Double>] = []
    private var droppedThrough = -Double.infinity
    private var capturePending = false
    private var interrupted = false
    private var retentionSeconds = 10.0
    private let limit = 12_000

    func setActive(_ active: Bool) {
        guard active != self.active else { return }
        self.active = active
        if active && manager == nil { manager = CBCentralManager(delegate: self, queue: .main, options: [CBCentralManagerOptionShowPowerAlertKey: false]) }
        updateScan()
        if !active && !capturePending { reset() }
    }

    func configure(retention: Double) {
        retentionSeconds = retention
    }

    func begin() {
        capturePending = true
        interrupted = false
    }

    func finish(start: Double, end: Double) -> RadioFingerprint {
        var intervals = coverage
        if let scanningSince { intervals.append(scanningSince...max(scanningSince, CACurrentMediaTime())) }
        let scanned = intervals.reduce(0.0) { $0 + max(0, min($1.upperBound, end) - max($1.lowerBound, start)) }
        let result = RadioFingerprintBuilder.build(observations, start: start, end: end, coverage: scanned,
                                                    availability: availability, dropped: droppedThrough >= start, interrupted: interrupted)
        capturePending = false
        reset()
        return result
    }

    private func reset() {
        observations.removeAll(keepingCapacity: false)
        coverage.removeAll(keepingCapacity: false)
        if scanningSince != nil { scanningSince = CACurrentMediaTime() }
        droppedThrough = -Double.infinity
        secret = SymmetricKey(size: .bits256)
    }

    private var availability: String {
        guard let state = manager?.state else { return "notStarted" }
        switch state {
        case .poweredOn: return active ? "available" : "inactive"
        case .poweredOff: return "poweredOff"
        case .unauthorized: return "permissionUnavailable"
        case .unsupported: return "unsupported"
        case .resetting, .unknown: return "initializing"
        @unknown default: return "unknown"
        }
    }

    private func updateScan() {
        guard let manager else { return }
        let now = CACurrentMediaTime()
        if active && manager.state == .poweredOn {
            if scanningSince == nil {
                scanningSince = now
                manager.scanForPeripherals(withServices: nil, options: [CBCentralManagerScanOptionAllowDuplicatesKey: true])
            }
        } else if let start = scanningSince {
            coverage.append(start...now)
            scanningSince = nil
            if manager.isScanning { manager.stopScan() }
            if capturePending { interrupted = true }
        }
        coverage.removeAll { $0.upperBound < now - retentionSeconds }
    }

    func centralManagerDidUpdateState(_ central: CBCentralManager) { updateScan() }

    func centralManager(_ central: CBCentralManager, didDiscover peripheral: CBPeripheral,
                        advertisementData: [String: Any], rssi RSSI: NSNumber) {
        guard active, scanningSince != nil else { return }
        let now = CACurrentMediaTime()
        let cutoff = now - retentionSeconds
        if let first = observations.firstIndex(where: { $0.time >= cutoff }) {
            if first > 0 { observations.removeFirst(first) }
        } else { observations.removeAll(keepingCapacity: true) }
        if observations.count >= limit {
            droppedThrough = observations[999].time
            observations.removeFirst(1000)
        }
        var uuid = peripheral.identifier.uuid
        let identifier = withUnsafeBytes(of: &uuid) { Data(HMAC<SHA256>.authenticationCode(for: $0, using: secret)) }
        let rssi = RSSI.intValue
        let tx = (advertisementData[CBAdvertisementDataTxPowerLevelKey] as? NSNumber)?.intValue
        observations.append(.init(identifier: identifier, time: now, rssi: (-127...20).contains(rssi) ? rssi : nil,
                                  txPower: tx.flatMap { (-127...20).contains($0) ? $0 : nil }))
    }
}
