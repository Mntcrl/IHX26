import Foundation
import SwiftUI

struct CertificationScreen: View {
    @Environment(\.dismiss) private var dismiss
    @Bindable var model: CertificationController
    @State private var galleryLibrary = GalleryLibrary()

    var body: some View {
        NavigationStack {
            Form {
                if let message = model.message { Section { Text(message) } }
                if let busy = model.busy {
                    Section {
                        ProgressView(busy)
                        if let progress = model.progress {
                            LabeledContent("Prover phase", value: progress.phase)
                            if let seconds = progress.seconds {
                                LabeledContent("Phase time", value: seconds.formatted(.number.precision(.fractionLength(2))) + " s")
                            }
                        }
                        Button("Pause and keep progress", action: model.pause)
                    }
                }
                Section("Circuit and network") {
                    LabeledContent("Network", value: "XRPL Testnet")
                    if let pin = model.pin {
                        LabeledContent("Proof setup", value: pin.ceremony == "development-single-party" ? "Development setup (Testnet)" : pin.ceremony)
                        LabeledContent("CIRCUIT transaction", value: pin.settings.circuitRegistrationTx)
                            .font(.footnote.monospaced()).textSelection(.enabled)
                        LabeledContent("Maximum fee per transaction", value: "\(pin.settings.xrpl.maxFeeDrops) drops")
                        if let build = model.build { LabeledContent("App Attest environment", value: build.environmentName) }
                    }
                    if let detail = model.artifactMessage { Text(detail) }
                    else if model.artifactsReady { Label("Matched proving files ready", systemImage: "checkmark.circle") }
                    Button("Reload setup and proving files", action: model.unlockRecovery).disabled(!model.idle)
                }
                walletSection
                registrationSection
                Section("Capture settings") {
                    Picker("Image profile", selection: $model.imageFormat) {
                        ForEach(CaptureImageFormat.allCases) { Text($0.rawValue).tag($0) }
                    }
                    Toggle("Keep private evidence after certification", isOn: $model.keepEvidence)
                    Text("ARKit, LiDAR and motion evidence stays on this phone. Only protocol transactions are published. A normal handheld capture about 0.5–3 meters from a three-dimensional scene works best. Hold naturally through the shutter.")
                        .font(.footnote)
                    Text("Certification tests physical consistency under one policy; it does not establish scene identity, time or provenance beyond the protocol’s ledger and device checks.")
                        .font(.footnote)
                }
                if let pending = model.pending {
                    Section("In progress") {
                        LabeledContent("State", value: pending.state.rawValue)
                        Button("Resume this capture", action: model.resume).disabled(!model.idle || !model.loaded)
                    }
                }
                let rows = model.records.filter { $0.certification != nil }
                if !rows.isEmpty {
                    Section("Capture history") {
                        ForEach(rows) { record in
                            NavigationLink {
                                GalleryDetail(model: model, library: galleryLibrary, item: .attempt(record))
                            } label: {
                                VStack(alignment: .leading) {
                                    Text(record.state.rawValue)
                                    Text(galleryDate(record.claimedTimeMs), format: .dateTime.day().month().hour().minute())
                                        .font(.caption).foregroundStyle(.secondary)
                                }
                            }
                        }
                    }
                }
            }
            .navigationTitle("Galaxy Certification")
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
        }
    }

    @ViewBuilder private var walletSection: some View {
        Section("Testnet wallet") {
            if let wallet = model.wallet {
                Text(wallet.classicAddress).font(.footnote.monospaced()).textSelection(.enabled)
                if let funding = model.funding {
                    LabeledContent("Validated balance", value: funding.balanceDrops.map(xrp) ?? "Not funded")
                    LabeledContent("Reserve", value: xrp(funding.reserveDrops))
                }
                Button("Request XRP from Testnet faucet", action: model.fundWallet).disabled(!model.idle || !model.loaded)
                Button("Refresh validated balance", action: model.refreshFunding).disabled(!model.idle || !model.loaded)
            } else {
                Button("Create Testnet wallet", action: model.createWallet).disabled(!model.idle || !model.loaded)
            }
            Text("Fund the wallet before device registration. The app waits for validated funds above the account reserve to cover registration fees. Only the public address is sent to the faucet.")
                .font(.footnote)
        }
    }

    @ViewBuilder private var registrationSection: some View {
        Section("Device registration") {
            if let registration = model.registration {
                if let tx = registration.registrationTx {
                    LabeledContent("DEVICE transaction", value: tx).font(.footnote.monospaced()).textSelection(.enabled)
                }
                if registration.preparationLedger != nil { Label("Wallet prepared", systemImage: "checkmark.circle") }
                if !registration.chunks.isEmpty {
                    LabeledContent("Validated registration chunks", value: "\(registration.chunkLedgers.count)/\(registration.chunks.count)")
                }
                if let failure = registration.failure { Text(failure) }
                let transactions = registration.allTransactions
                if !transactions.isEmpty {
                    LabeledContent("Journaled transactions", value: "\(transactions.count)")
                    LabeledContent("Signed size / fees", value: "\(transactions.reduce(0) { $0 + $1.blob.count }) bytes / \(transactions.reduce(UInt64(0)) { $0 + $1.feeDrops }) drops")
                }
            }
            if !model.hasRegistration {
                Button(model.registration?.failure != nil ? "Start a new device registration" :
                        model.registration == nil ? "Prepare wallet and register device" : "Resume device registration",
                       action: model.register)
                    .disabled(!model.readyToRegister)
            }
            Text("Registration enables tracking of the wallet’s own transactions, then publishes Apple’s device credential to Testnet. Incoming XRP top-ups do not change that chain.")
                .font(.footnote)
        }
    }

    private func xrp(_ drops: UInt64) -> String {
        (Decimal(drops) / 1_000_000).formatted(.number.precision(.fractionLength(0...6))) + " XRP"
    }
}
