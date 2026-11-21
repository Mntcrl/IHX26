import Foundation
import SwiftUI
import UniformTypeIdentifiers

struct CertificationScreen: View {
    @Environment(\.dismiss) private var dismiss
    @Bindable var model: CertificationController

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
                                CaptureResultScreen(record: record, model: model)
                            } label: {
                                VStack(alignment: .leading) {
                                    Text(record.state.rawValue)
                                    Text(Date(timeIntervalSince1970: Double(record.claimedTimeMs) / 1_000), format: .dateTime.day().month().hour().minute())
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

private struct CaptureResultScreen: View {
    let record: CaptureRecord
    let model: CertificationController
    @State private var file: CertifiedFile?
    @State private var showFiles = false
    @State private var savingToPhotos = false
    @State private var message: String?

    private var row: CaptureRecord { model.records.first { $0.id == record.id } ?? record }

    var body: some View {
        Form {
            Section("Result") {
                LabeledContent("State", value: row.state.rawValue)
                if row.state == .certified {
                    if let pin = model.pin, pin.id == row.certification?.descriptorId, pin.ceremony == "development-single-party" {
                        Text("Development setup (Testnet)").font(.footnote)
                    }
                    Text("Physical consistency certified under the pinned Testnet policy. Export the original file to preserve verification references.")
                }
                ForEach(row.reasons, id: \.self) { reason in Text(reason) }
                if let journal = row.certification {
                    LabeledContent("Circuit", value: journal.descriptorId).font(.caption.monospaced()).textSelection(.enabled)
                    LabeledContent("Image profile", value: journal.imageFormat.rawValue)
                }
            }
            if let journal = row.certification {
                Section("XRPL references") {
                    LabeledContent("Pre-capture ledger", value: String(journal.preledger.index))
                    if let capture = journal.capture { reference("CAPTURE", capture, ledger: journal.captureLedger) }
                    if let selection = journal.selection { LabeledContent("Selection ledger", value: String(selection.index)) }
                    if let proof = journal.proof { reference("PROOF", proof, ledger: journal.proofLedger) }
                }
                if let witness = journal.witnessSeconds, let prove = journal.proveSeconds {
                    Section("On-device timings") {
                        LabeledContent("Witness generation", value: seconds(witness))
                        LabeledContent("Proof generation", value: seconds(prove))
                        if let start = journal.enteredAt[CaptureState.acquiring.rawValue],
                           let end = journal.enteredAt[CaptureState.certified.rawValue] {
                            LabeledContent("Shutter to certification (wall clock)", value: seconds(end.timeIntervalSince(start)))
                        }
                    }
                }
            }
            if let url = model.certifiedURL(row) {
                Section("Original certified image") {
                    Button("Export to Files") {
                        do { file = try CertifiedFile(url: url); showFiles = true }
                        catch { message = "The protected certified file could not be read." }
                    }
                    Button("Save original to Photos") {
                        savingToPhotos = true
                        Task { @MainActor in
                            defer { savingToPhotos = false }
                            do { try await ExportService.saveToPhotos(url); message = "The original image was added to Photos." }
                            catch let error as FlowError { message = FlowError.message(error) }
                            catch { message = "Photos could not save the original image. Try Files instead." }
                        }
                    }.disabled(savingToPhotos)
                }
            } else if row.state == .certified {
                Text("The certified image file is unavailable. Unlock the device and reopen the app.")
            }
            if let message { Section { Text(message) } }
        }
        .navigationTitle("Capture Result")
        .fileExporter(isPresented: $showFiles, document: file,
                      contentType: row.certification?.imageFormat == .jpeg ? .jpeg : .png,
                      defaultFilename: "Galaxy-\(row.id.uuidString)") { result in
            file = nil
            switch result {
            case .success: message = "Exported the original certified image."
            case .failure: message = "Files did not complete the export."
            }
        }
    }

    @ViewBuilder private func reference(_ name: String, _ signed: JournalTransaction, ledger: UInt32?) -> some View {
        VStack(alignment: .leading) {
            Text(name + (ledger.map { " · ledger \($0)" } ?? " · no validated success"))
            Text(signed.hash).font(.caption.monospaced()).textSelection(.enabled)
            Text("\(signed.blob.count) bytes · \(signed.feeDrops) drops").font(.caption)
        }
    }

    private func seconds(_ value: Double) -> String { value.formatted(.number.precision(.fractionLength(2))) + " s" }
}
