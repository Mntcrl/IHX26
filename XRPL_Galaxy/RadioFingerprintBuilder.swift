import Foundation

nonisolated struct RadioFingerprint: Codable, Sendable {
    let availability: String
    let complete: Bool
    let scanDurationMilliseconds: Int
    let observationCallbacks: Int
    let observedPeripheralIdentifiers: Int
    let rssiHistogram: [Int]
    let medianRSSIBucketDBm: Int?
    let rssiStdDevBucketDB: Int?
    let temporalDensityBySecond: [Int]
    let persistenceHistogram: [Int]
    let callbacksWithAdvertisedTxPower: Int
}

nonisolated enum RadioFingerprintBuilder {
    struct Observation {
        let identifier: Data
        let time: Double
        let rssi: Int?
        let txPower: Int?
    }

    static func build(_ observations: [Observation], start: Double, end: Double,
                      coverage: Double, availability: String, dropped: Bool, interrupted: Bool) -> RadioFingerprint {
        let window = observations.filter { $0.time >= start && $0.time <= end }
        let valid = window.compactMap(\.rssi).sorted()
        let histogram = valid.reduce(into: Array(repeating: 0, count: 10)) {
            $0[min(9, max(0, ($1 + 110) / 10))] += 1
        }
        let mean = valid.isEmpty ? 0 : Double(valid.reduce(0, +)) / Double(valid.count)
        let variance = valid.isEmpty ? 0 : valid.reduce(0.0) { $0 + pow(Double($1) - mean, 2) } / Double(valid.count)
        let groups = Dictionary(grouping: window, by: \.identifier)
        var persistence = Array(repeating: 0, count: 4)
        for values in groups.values {
            let span = (values.last?.time ?? 0) - (values.first?.time ?? 0)
            let bucket = values.count == 1 ? 0 : span < 1 ? 1 : span < 3 ? 2 : 3
            persistence[bucket] += 1
        }
        var density = Array(repeating: 0, count: max(1, Int(ceil(end - start))))
        for item in window { density[min(density.count - 1, Int(item.time - start))] += 1 }
        return RadioFingerprint(availability: availability, complete: !dropped && !interrupted && coverage >= end - start - 0.15,
                                scanDurationMilliseconds: Int((coverage * 10).rounded()) * 100,
                                observationCallbacks: window.count, observedPeripheralIdentifiers: groups.count,
                                rssiHistogram: histogram,
                                medianRSSIBucketDBm: valid.isEmpty ? nil : Int((Double(valid[valid.count / 2]) / 5).rounded()) * 5,
                                rssiStdDevBucketDB: valid.isEmpty ? nil : Int((sqrt(variance) / 5).rounded()) * 5,
                                temporalDensityBySecond: density, persistenceHistogram: persistence,
                                callbacksWithAdvertisedTxPower: window.filter { $0.txPower != nil }.count)
    }
}

