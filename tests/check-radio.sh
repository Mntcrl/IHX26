#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
cat > "$scratch/main.swift" <<'SWIFT'
import Foundation
let absent = RadioFingerprintBuilder.build([], start: 0, end: 10, coverage: 0,
                                          availability: "poweredOff", dropped: false, interrupted: false)
precondition(!absent.complete && absent.observationCallbacks == 0 && absent.availability == "poweredOff")
let quiet = RadioFingerprintBuilder.build([], start: 0, end: 10, coverage: 10,
                                         availability: "available", dropped: false, interrupted: false)
precondition(quiet.complete && quiet.observedPeripheralIdentifiers == 0)
let a = Data([123]), b = Data([231])
let radio = RadioFingerprintBuilder.build([
    .init(identifier: a, time: -1, rssi: -90, txPower: nil),
    .init(identifier: a, time: 1, rssi: -67, txPower: -12),
    .init(identifier: b, time: 2, rssi: nil, txPower: nil),
    .init(identifier: a, time: 5, rssi: -81, txPower: nil)
], start: 0, end: 10, coverage: 9, availability: "available", dropped: false, interrupted: true)
precondition(!radio.complete && radio.observationCallbacks == 3 && radio.observedPeripheralIdentifiers == 2)
precondition(radio.rssiHistogram.reduce(0, +) == 2 && radio.persistenceHistogram == [1, 0, 0, 1])
precondition(radio.callbacksWithAdvertisedTxPower == 1 && radio.scanDurationMilliseconds == 9000)
let value = try JSONSerialization.jsonObject(with: JSONEncoder().encode(radio)) as! [String: Any]
precondition(value.keys.allSatisfy { !$0.contains("timestamp") && !$0.contains("UUID") })
print("Radio windowing, deduplication, missing Bluetooth, and zero observations passed.")
SWIFT
swiftc "$root/XRPL_Galaxy/RadioFingerprintBuilder.swift" "$scratch/main.swift" -o "$scratch/verify"
"$scratch/verify"
