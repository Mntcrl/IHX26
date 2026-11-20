import Foundation

// Wire types for the POV committee (plan contracts C2, C8, C11).
// Property names equal the JSON keys; no key-coding strategies are used.

nonisolated struct T0: Codable, Sendable, Equatable {
    let index: UInt32
    let hash: String
}

/// C2 manifest, signed by the device over the exact encoded bytes.
nonisolated struct Manifest: Codable, Sendable {
    let v: Int
    let app: String
    let committee: String
    let t0: T0
    let captured_at: String
    let image_sha256: String
    let depth_sha256: String
    let evidence_root: String
    let grid: [Int]
    let shares_sha256: [String]
    let device_pubkey: String
}

nonisolated struct EncShare: Codable, Sendable {
    let node_id: Int
    let enc_b64: String
    let ct_b64: String
}

nonisolated struct CaptureRequest: Codable, Sendable {
    let manifest_b64: String
    let signature_hex: String
    let shares: [EncShare]
}

nonisolated struct NodeResult: Codable, Sendable, Identifiable {
    let id: Int
    let ok: Bool
    let reason: String?
}

/// C8 Attempt: the committee's verdict on one capture.
nonisolated struct Attempt: Codable, Sendable, Identifiable {
    let capture_id: String
    let at: Double
    let verdict: String
    let reason: String?
    let rel_rms: Double?
    let frac: Double?
    let nodes: [NodeResult]
    let signers: [Int]
    let t0: UInt32?
    let t1: UInt32?
    let tx_hash: String?
    let explorer_url: String?
    let image_sha256: String?
    let device_account: String?
    let published: String?

    var id: String { capture_id }
    var passed: Bool { verdict == "PASS" }
}

nonisolated struct ConfigNode: Codable, Sendable, Identifiable {
    let id: Int
    let account: String
    let hpke_pk: String
}

nonisolated struct ConfigResponse: Codable, Sendable {
    let committee_account: String
    let group_pk: String
    let threshold: Int
    let n: Int
    let xrpl_rpc: String
    let xrpl_wss: String
    let explorer: String
    let genesis_tx: String?
    let nodes: [ConfigNode]
    let min_rel_rms: Double
    let min_frac: Double
}

nonisolated struct PublishRequest: Codable, Sendable {
    let capture_id: String
    let image_b64: String
}

nonisolated struct PublishResponse: Codable, Sendable {
    let url: String
}

// XRPL JSON-RPC result wrappers (api_version 2).

nonisolated struct RPCLedgerResponse: Codable, Sendable {
    nonisolated struct Result: Codable, Sendable {
        let ledger_index: UInt32?
        let ledger_hash: String?
        let validated: Bool?
        let status: String?
        let error: String?
    }
    let result: Result
}

nonisolated struct RPCLedgerEntryResponse: Codable, Sendable {
    nonisolated struct Node: Codable, Sendable {
        let Data: String?
    }
    nonisolated struct Result: Codable, Sendable {
        let node: Node?
        let status: String?
        let error: String?
    }
    let result: Result
}

nonisolated enum POVError: Error, LocalizedError {
    case badScan(String)
    case notReady(String)
    case http(Int)

    var errorDescription: String? {
        switch self {
        case .badScan(let why): return "Bad scan: \(why)"
        case .notReady(let why): return why
        case .http(let status): return "HTTP \(status)"
        }
    }
}
