import SwiftUI
import UIKit

private enum POVStyle {
    static let pass = Color(red: 0.78, green: 0.98, blue: 0.36)
    static let reject = Color(red: 1.0, green: 0.36, blue: 0.33)
    static let muted = Color.white.opacity(0.56)
}

private func ledgerText(_ index: UInt32?) -> String {
    index.map { String($0) } ?? "—"
}

private func shortAddress(_ address: String) -> String {
    address.count > 12 ? "\(address.prefix(6))…\(address.suffix(4))" : address
}

/// Header pill: committee account and whether its node keys were confirmed on XRPL.
struct POVStatusPill: View {
    @ObservedObject var pov: POVModel

    private var dotColor: Color {
        if pov.busy { return .yellow }
        if pov.config != nil && pov.keysOK == true { return POVStyle.pass }
        return .orange
    }

    private var label: String {
        if pov.busy { return pov.step }
        guard let config = pov.config else { return pov.step }
        let committee = "COMMITTEE \(shortAddress(config.committee_account))"
        switch pov.keysOK {
        case true?: return "\(committee) · keys ✓ on XRPL"
        case false?: return "\(committee) · keys ✗ on XRPL"
        case nil: return "\(committee) · \(pov.step)"
        }
    }

    var body: some View {
        Button { pov.showSettings = true } label: {
            HStack(spacing: 7) {
                Circle().fill(dotColor).frame(width: 6, height: 6)
                if pov.busy { ProgressView().controlSize(.mini).tint(.white) }
                Text(label)
                    .font(.system(size: 9, weight: .semibold, design: .monospaced))
                    .lineLimit(1)
            }
            .foregroundStyle(.white)
            .padding(.horizontal, 11).padding(.vertical, 8)
            .background(.black.opacity(0.52), in: Capsule())
            .overlay(Capsule().stroke(.white.opacity(0.15), lineWidth: 1))
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Verification status: \(label)")
    }
}

/// Committee verdict for one capture.
struct POVResultSheet: View {
    @ObservedObject var pov: POVModel
    let attempt: Attempt
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                Section {
                    VStack(spacing: 6) {
                        Image(systemName: attempt.passed ? "checkmark.seal.fill" : "xmark.octagon.fill")
                            .font(.system(size: 52))
                        Text(attempt.passed ? "PASS" : "REJECT")
                            .font(.system(size: 40, weight: .heavy, design: .rounded))
                        if let reason = attempt.reason {
                            Text(reason).font(.system(.callout, design: .monospaced))
                        }
                        Text(attempt.passed ? "Anchored on XRPL by the committee" : "Not anchored")
                            .font(.footnote).foregroundStyle(.secondary)
                    }
                    .foregroundStyle(attempt.passed ? POVStyle.pass : POVStyle.reject)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 8)
                }

                Section("Depth evidence (MPC)") {
                    row("rel_rms", attempt.rel_rms.map { String(format: "%.4f", $0) } ?? "—")
                    row("frac", attempt.frac.map { String(format: "%.3f", $0) } ?? "—")
                }

                Section("Committee nodes") {
                    ForEach(attempt.nodes) { node in
                        HStack {
                            Image(systemName: node.ok ? "checkmark.circle.fill" : "xmark.circle.fill")
                                .foregroundStyle(node.ok ? POVStyle.pass : POVStyle.reject)
                            Text("Node \(node.id)")
                            if attempt.signers.contains(node.id) {
                                Text("signer").font(.caption2.bold()).padding(.horizontal, 6).padding(.vertical, 2)
                                    .background(.secondary.opacity(0.2), in: Capsule())
                            }
                            Spacer()
                            Text(node.reason ?? (node.ok ? "ok" : "—"))
                                .font(.system(.caption, design: .monospaced)).foregroundStyle(.secondary)
                        }
                    }
                }

                Section("XRPL") {
                    row("T0 → T1", "\(ledgerText(attempt.t0)) → \(ledgerText(attempt.t1))")
                    if let device = attempt.device_account { row("Device", shortAddress(device)) }
                    if let hash = attempt.tx_hash { row("Anchor tx", shortAddress(hash)) }
                    if let link = attempt.explorer_url, let url = URL(string: link) {
                        Link(destination: url) { Label("Open in XRPL explorer", systemImage: "arrow.up.right.square") }
                    }
                }

                if attempt.passed {
                    Section {
                        if let published = pov.published {
                            Label("Published: \(published)", systemImage: "checkmark.icloud")
                                .font(.footnote).textSelection(.enabled)
                        } else {
                            Button {
                                Task { await pov.publish() }
                            } label: {
                                HStack {
                                    Label("Publish photo", systemImage: "square.and.arrow.up")
                                    if pov.publishing { Spacer(); ProgressView() }
                                }
                            }
                            .disabled(pov.publishing)
                        }
                    } footer: {
                        Text("Uploads only the JPEG so anyone can check it against the anchor. Sensor evidence stays on this phone.")
                    }
                }
            }
            .navigationTitle("Photo origin")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
        }
        .preferredColorScheme(.dark)
    }

    private func row(_ title: String, _ value: String) -> some View {
        HStack {
            Text(title)
            Spacer()
            Text(value).font(.system(.body, design: .monospaced)).foregroundStyle(.secondary)
        }
    }
}

/// Coordinator URL and the device public key used for enrollment.
struct POVSettingsSheet: View {
    @ObservedObject var pov: POVModel
    @State private var draftURL = ""
    @State private var copied = false
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Form {
                Section("Committee coordinator") {
                    TextField("http://10.164.228.104:8088", text: $draftURL)
                        .keyboardType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .font(.system(.body, design: .monospaced))
                    Button("Save & reload") { pov.saveSettings(coordinatorURL: draftURL) }
                        .disabled(draftURL.trimmingCharacters(in: .whitespaces).isEmpty)
                }

                Section {
                    Text(pov.devicePubkey.isEmpty ? "unavailable" : pov.devicePubkey)
                        .font(.system(.footnote, design: .monospaced))
                        .textSelection(.enabled)
                    Button(copied ? "Copied" : "Copy") {
                        UIPasteboard.general.string = pov.devicePubkey
                        copied = true
                    }
                    .disabled(pov.devicePubkey.isEmpty)
                } header: {
                    Text("Device key (Secure Enclave P-256)")
                } footer: {
                    Text("Send this key to the operator for enrollment (device DID + manufacturer credential on XRPL).")
                }

                Section("Status") {
                    if let config = pov.config {
                        LabeledContent("Committee", value: shortAddress(config.committee_account))
                        LabeledContent("Threshold", value: "\(config.threshold)-of-\(config.n)")
                    }
                    Text(pov.step).font(.footnote)
                }
            }
            .navigationTitle("Verification settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
            .onAppear { draftURL = pov.coordinatorURL }
        }
    }
}
