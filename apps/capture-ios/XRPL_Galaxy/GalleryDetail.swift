import SwiftUI
import UIKit

struct GalleryDetail: View {
    let model: CertificationController
    let library: GalleryLibrary
    let item: GalleryItem

    @State private var preview: UIImage?
    @State private var references: CertifiedPhotoDetails?
    @State private var inclusionTime: UInt64?
    @State private var imageUnavailable = false
    @State private var exportTask: Task<Void, Never>?
    @State private var prepared: [CaptureImageFormat: URL] = [:]
    @State private var file: GalleryImageFile?
    @State private var fileFormat: CaptureImageFormat = .jpeg
    @State private var fileName = ""
    @State private var showFiles = false
    @State private var shareFile: SharedPhoto?
    @State private var message: String?

    private enum Destination {
        case share, photos, files
    }

    private var record: CaptureRecord? {
        guard case .attempt(let row) = item else { return nil }
        return model.records.first { $0.id == row.id } ?? row
    }

    private var source: GalleryImageSource? {
        switch item {
        case .calibration(let photo): photo.imageSource
        case .attempt: record.flatMap { model.galleryImage($0) }
        }
    }

    var body: some View {
        Form {
            if source != nil {
                Section {
                    if let preview {
                        Image(uiImage: preview).resizable().scaledToFit()
                            .accessibilityLabel(record?.state == .certified ? "Certified photo" : "Not certified photo")
                    } else if imageUnavailable {
                        Label("Photo unavailable", systemImage: "photo.badge.exclamationmark")
                    } else { ProgressView("Loading photo") }
                }
                .listRowInsets(EdgeInsets())
            }
            if let record {
                result(record)
                if let journal = record.certification { ledgerReferences(journal) }
            } else if case .calibration(let photo) = item {
                Section("Calibration capture") {
                    GalleryBadge(state: nil)
                    LabeledContent("Capture time", value: galleryDate(photo.claimedTimeMs).formatted())
                    LabeledContent("Scene", value: photo.label.title)
                    Text("Private calibration capture. Only the plain image is offered for export; LiDAR and other raw sensor data stay on this phone.")
                        .font(.footnote)
                }
            }
            if let source, preview != nil {
                if source.certified {
                    if references != nil {
                        if source.format == .jpeg {
                            exportSection(.jpeg, title: "Certified JPEG · original", source: source)
                        } else {
                            Section {
                                Text("Certified JPEG is unavailable for a PNG-profile capture because JPEG encoding can change the certified pixels.")
                                    .font(.footnote)
                            }
                        }
                        exportSection(.png, title: "Certified PNG · lossless", source: source)
                    } else {
                        Section { Text("The stored certification references could not be read. No certified export is available.") }
                    }
                } else {
                    exportSection(.jpeg, title: "Plain JPEG · not certified", source: source)
                }
            }
            if exportTask != nil { Section { ProgressView("Preparing photo") } }
            if let message { Section { Text(message) } }
        }
        .navigationTitle(record == nil ? "Calibration Photo" : "Capture Result")
        .navigationBarTitleDisplayMode(.inline)
        .task(id: source) { await loadPhoto(source) }
        .onDisappear { exportTask?.cancel() }
        .sheet(item: $shareFile) { SharePhotoSheet(file: $0.url) }
        .fileExporter(isPresented: $showFiles, document: file, contentType: fileFormat.contentType,
                      defaultFilename: fileName) { result in
            file = nil
            switch result {
            case .success: message = "Photo saved to Files."
            case .failure: message = "Files did not complete the export."
            }
        }
    }

    @ViewBuilder private func result(_ row: CaptureRecord) -> some View {
        Section("Result") {
            GalleryBadge(state: row.state)
            LabeledContent("Capture time (device)", value: galleryDate(row.claimedTimeMs).formatted())
            if row.state == .certified {
                Text("Physical consistency certified under the pinned Testnet policy. This does not establish scene identity or provenance.")
                    .font(.footnote)
                if source == nil { Text("The certified file is unavailable. Unlock the phone and reopen the app.") }
                if let inclusionTime {
                    LabeledContent("Capture ledger close time",
                                   value: Date(timeIntervalSince1970: Double(inclusionTime)).formatted())
                }
                if let references { textID("Photo ID", references.photoId) }
            }
            ForEach(Array(row.reasons.enumerated()), id: \.offset) { _, reason in Text(reason) }
            if let journal = row.certification {
                LabeledContent("Committed profile", value: journal.imageFormat == .jpeg ? "jpeg-v1" : "png-v1")
                textID("Circuit ID", journal.descriptorId)
                if let pin = model.pin, pin.id == journal.descriptorId {
                    textID("Policy ID", pin.policyID)
                    LabeledContent("Proof setup", value: pin.ceremony == "development-single-party" ? "Development setup (Testnet)" : pin.ceremony)
                }
                if let environment = environment(journal) { LabeledContent("Attestation environment", value: environment) }
            }
        }
    }

    @ViewBuilder private func ledgerReferences(_ journal: CertificationJournal) -> some View {
        Section("XRPL Testnet") {
            LabeledContent("Pre-capture ledger", value: String(journal.preledger.index))
            if let capture = journal.capture {
                transaction("CAPTURE", capture, ledgerLabel: "L0", ledger: journal.captureLedger, link: references?.captureTxUrl)
            }
            if let selection = journal.selection {
                LabeledContent("Selection ledger (Lsel)", value: String(selection.index))
            }
            if let proof = journal.proof {
                transaction("PROOF", proof, ledgerLabel: "Lproof", ledger: journal.proofLedger, link: references?.proofTxUrl)
            }
        }
        if let witness = journal.witnessSeconds, let prove = journal.proveSeconds {
            Section("On-device timings") {
                LabeledContent("Witness generation", value: seconds(witness))
                LabeledContent("Proof generation", value: seconds(prove))
                if let start = journal.enteredAt[CaptureState.acquiring.rawValue],
                   let end = journal.enteredAt[CaptureState.certified.rawValue] {
                    LabeledContent("Shutter to certification", value: seconds(end.timeIntervalSince(start)))
                }
            }
        }
    }

    private func transaction(_ title: String, _ tx: JournalTransaction, ledgerLabel: String,
                             ledger: UInt32?, link: String?) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(title + (ledger.map { " · \(ledgerLabel) \($0)" } ?? " · not validated"))
            Text(tx.hash).font(.caption.monospaced()).textSelection(.enabled)
            Text("\(tx.blob.count) bytes · \(tx.feeDrops) drops").font(.caption).foregroundStyle(.secondary)
            if let link, let url = URL(string: link), url.scheme == "https", url.lastPathComponent.uppercased() == tx.hash {
                Link("View \(title) on Testnet Explorer", destination: url)
            }
            Button("Copy \(title) ID", systemImage: "doc.on.doc") {
                UIPasteboard.general.string = tx.hash
                message = "Copied \(title) transaction ID."
            }.buttonStyle(.borderless)
        }
    }

    private func exportSection(_ format: CaptureImageFormat, title: String, source: GalleryImageSource) -> some View {
        Section(title) {
            Button("Share \(format.rawValue)", systemImage: "square.and.arrow.up") { export(format, from: source, to: .share) }
            Button("Save \(format.rawValue) to Photos", systemImage: "photo") { export(format, from: source, to: .photos) }
            Button("Save \(format.rawValue) to Files", systemImage: "folder") { export(format, from: source, to: .files) }
        }.disabled(exportTask != nil)
    }

    private func loadPhoto(_ source: GalleryImageSource?) async {
        exportTask?.cancel()
        preview = nil
        references = nil
        imageUnavailable = false
        inclusionTime = nil
        prepared = [:]
        guard let source else { return }
        do {
            let data = try await library.image(source)
            try Task.checkCancellation()
            preview = UIImage(data: data)
            imageUnavailable = preview == nil
            if source.certified, let journal = record?.certification {
                let details = try await library.certifiedReferences(data)
                try Task.checkCancellation()
                guard details.imageProfile == source.format.profile,
                      let capture = journal.capture, let proof = journal.proof,
                      Self.link(details.captureTxUrl, matches: capture.hash) != nil,
                      Self.link(details.proofTxUrl, matches: proof.hash) != nil else { return }
                references = details
                if let closeTime = journal.captureCloseTime {
                    inclusionTime = closeTime
                } else if let index = journal.captureLedger {
                    let closeTime = try await model.captureCloseTime(index)
                    try Task.checkCancellation()
                    inclusionTime = closeTime
                }
            }
        } catch is CancellationError {
        } catch { imageUnavailable = preview == nil }
    }

    private func export(_ format: CaptureImageFormat, from source: GalleryImageSource, to destination: Destination) {
        guard exportTask == nil else { return }
        message = nil
        exportTask = Task {
            defer { exportTask = nil }
            do {
                let url: URL
                if let cached = prepared[format], FileManager.default.fileExists(atPath: cached.path) {
                    url = cached
                } else {
                    url = try await prepare(format, from: source)
                    try Task.checkCancellation()
                    prepared[format] = url
                }
                try Task.checkCancellation()
                switch destination {
                case .share: shareFile = SharedPhoto(url: url)
                case .photos:
                    try await ExportService.saveToPhotos(url)
                    message = "Photo added to Photos."
                case .files:
                    file = try GalleryImageFile(url: url)
                    fileFormat = format
                    fileName = url.deletingPathExtension().lastPathComponent
                    showFiles = true
                }
            } catch is CancellationError {
            } catch let error as FlowError { message = FlowError.message(error) }
            catch { message = "Could not export this photo. Unlock the phone and try again." }
        }
    }

    private func prepare(_ format: CaptureImageFormat, from source: GalleryImageSource) async throws -> URL {
        guard source.certified else { return try await library.exportPlainJPEG(source) }
        guard let record, let journal = record.certification, let pin = model.pin, pin.id == journal.descriptorId,
              let captureLedger = journal.captureLedger, let selection = journal.selection,
              let proofLedger = journal.proofLedger, let environment = environment(journal),
              let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String else {
            throw FlowError.failed("The saved certification details are incomplete. No certified export was produced.")
        }
        let account = journal.accountAddress ?? model.wallet.flatMap { $0.accountId == journal.accountId ? $0.classicAddress : nil }
        guard let account else { throw FlowError.failed("The original Testnet account for this photo is unavailable.") }
        let closeTime: UInt64
        if let knownTime = journal.captureCloseTime ?? inclusionTime { closeTime = knownTime }
        else { closeTime = try await model.captureCloseTime(captureLedger) }
        let captureTime = Date(timeIntervalSince1970: Double(closeTime)).ISO8601Format()
        let claimedTime = galleryDate(record.claimedTimeMs).ISO8601Format(.init(includingFractionalSeconds: true))
        let info = ExportInfo(account: account, registrationTx: journal.deviceTx,
                              captureLedger: captureLedger, selectionLedger: selection.index, proofLedger: proofLedger,
                              captureTime: captureTime, claimedTime: claimedTime, circuitId: journal.descriptorId,
                              policyId: pin.policyID, policyName: pin.policyName, setupCeremony: pin.ceremony,
                              attestationEnvironment: environment, captureTxUrl: nil, proofTxUrl: nil, appVersion: version)
        return try await library.exportCertified(source, as: format, info: info)
    }

    private static func link(_ string: String, matches hash: String) -> URL? {
        guard let url = URL(string: string), url.scheme == "https", url.lastPathComponent.uppercased() == hash else { return nil }
        return url
    }

    private func environment(_ journal: CertificationJournal) -> String? {
        if let stored = journal.attestationEnvironment { return stored }
        guard let registration = model.registration, registration.registrationTx == journal.deviceTx else { return nil }
        return registration.build.environmentName
    }

    private func textID(_ title: String, _ value: String) -> some View {
        VStack(alignment: .leading) {
            Text(title)
            Text(value).font(.caption.monospaced()).textSelection(.enabled)
        }
    }

    private func seconds(_ value: Double) -> String { value.formatted(.number.precision(.fractionLength(2))) + " s" }
}

private struct SharedPhoto: Identifiable {
    let url: URL
    var id: URL { url }
}

private struct SharePhotoSheet: UIViewControllerRepresentable {
    let file: URL

    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: [file], applicationActivities: nil)
    }

    func updateUIViewController(_ controller: UIActivityViewController, context: Context) {}
}
