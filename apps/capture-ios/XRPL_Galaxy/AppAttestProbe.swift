#if DEBUG
import CryptoKit
import SwiftUI

struct AppAttestProbe: View {
    @State private var report: [String] = []

    var body: some View {
        VStack(alignment: .leading) {
            Text("App Attest device diagnostic").font(.headline)
            Text(report.joined(separator: "\n")).font(.system(.body, design: .monospaced))
        }
        .padding()
        .task {
            guard report.isEmpty else { return }
            await run()
            if ProcessInfo.processInfo.arguments.contains("--diagnostic-exit") {
                fflush(nil)
                exit(0)
            }
        }
    }

    private func note(_ line: String) {
        report.append(line)
        print("GXATTEST \(line)")
    }

    private func failure(_ operation: String, _ error: Error) {
        let error = error as NSError
        note("\(operation) failed: \(error.domain) \(error.code)")
    }

    private func run() async {
        note("isSupported=\(GalaxyAppAttest.isSupported)")
        let keyID: String
        do {
            keyID = try await GalaxyAppAttest.generateKeyForRegistration()
            note("generateKey succeeded")
        } catch {
            failure("generateKey", error)
            note("attestKey and generateAssertion require a generated key")
            return
        }
        do {
            let registration = try GalaxyAppAttest.registrationClientData(
                network: 1, ledger: 1234, ledgerHash: Data(repeating: 0xA5, count: 32),
                nonce: randomBytes(count: 32), keyID: keyID, account: Data(repeating: 0xB6, count: 20))
            // Two diagnostic-only capture cores so a verifier can check the assertion counter.
            let captures = try [Data(count: 299), Data(repeating: 0x01, count: 299)].map {
                try GalaxyAppAttest.assertionClientData(core: $0)
            }
            var attestation: Data?
            var assertions: [Data] = []
            do {
                let response = try await GalaxyAppAttest.attestDigest(keyID, digest: Data(SHA256.hash(data: registration)))
                attestation = response
                note("attestKey succeeded (\(response.count) bytes)")
            } catch { failure("attestKey", error) }
            for capture in captures {
                do {
                    let assertion = try await GalaxyAppAttest.assertDigest(keyID, digest: Data(SHA256.hash(data: capture)))
                    assertions.append(assertion)
                    note("generateAssertion #\(assertions.count) succeeded (\(assertion.count) bytes)")
                } catch {
                    failure("generateAssertion", error)
                    break
                }
            }
            guard attestation != nil || !assertions.isEmpty else { return }
            let dir = try FileManager.default.url(for: .documentDirectory, in: .userDomainMask,
                                                  appropriateFor: nil, create: true)
                .appending(path: "appattest/\(UUID().uuidString)", directoryHint: .isDirectory)
            try ProtectedFiles.directory(dir)
            try ProtectedFiles.create(Data(keyID.utf8), at: dir.appending(path: "key-id.txt"))
            try ProtectedFiles.create(registration, at: dir.appending(path: "registration-client-data.bin"))
            if let attestation { try ProtectedFiles.create(attestation, at: dir.appending(path: "attestation.cbor")) }
            for (index, assertion) in assertions.enumerated() {
                try ProtectedFiles.create(captures[index], at: dir.appending(path: "assertion-\(index + 1)-client-data.bin"))
                try ProtectedFiles.create(assertion, at: dir.appending(path: "assertion-\(index + 1).cbor"))
            }
            note("fixtures saved (dummy ledger, account and capture; not for certification)")
        } catch { failure("diagnostic", error) }
    }
}
#endif
