import SwiftUI
import UIKit

struct CaptureProgressScreen: View {
    let model: CertificationController
    /// Until this shutter produces a record, do not display the previous attempt.
    let excluding: UUID?

    @Environment(\.dismiss) private var dismiss
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var library = GalleryLibrary()
    @State private var preview: UIImage?
    @State private var details: CertifiedPhotoDetails?
    @State private var showTiles = false
    @State private var celebrate = false

    private var row: CaptureRecord? {
        let latest = model.pending ?? model.lastAttempt
        return latest?.id == excluding ? nil : latest
    }
    private var journal: CertificationJournal? { row?.certification }
    private var reveal: ChallengeReveal? {
        guard let selection = journal?.selection, let reveal = model.challengeReveal, reveal.ledger == selection.index else { return nil }
        return reveal
    }
    private var previewSource: GalleryImageSource? {
        guard let row, journal?.captureLedger != nil else { return nil }
        return model.galleryImage(row)
    }
    private var timeline: [Step] {
        func at(_ state: CaptureState) -> Date? { journal?.enteredAt[state.rawValue] }
        let witnessStart = at(.generatingWitness)
        let proofValidated = journal?.proofValidatedAt ?? at(.certified)
        return [
            Step("Sensors frozen", "The phone saves camera, LiDAR and motion readings from around the shutter.", at(.acquiring), at(.canonicalizing)),
            Step("Evidence committed (Poseidon)", "Poseidon fingerprints bind the evidence without publishing the sensor readings.", at(.canonicalizing) ?? at(.committing), at(.attesting)),
            Step("Apple App Attest", "The registered app key signs a statement tied to this capture.", at(.attesting), at(.submittingManifest)),
            Step("CAPTURE on XRPL", "Testnet validates a transaction that locks in the photo and its evidence commitments.", at(.submittingManifest), at(.waitingSelectionLedger), transaction: journal?.capture, ledger: journal?.captureLedger),
            Step("Waiting for the challenge ledger", "The challenge can’t be known until five more ledgers have validated.", at(.waitingSelectionLedger), witnessStart),
            Step("Challenge derived", "The validated ledger hash and capture ID determine which tiles are checked.", reveal?.startedAt ?? witnessStart, reveal?.revealedAt ?? at(.generatingProof)),
            Step("Witness", "The phone checks the sampled evidence and prepares private inputs for the circuit.", reveal?.revealedAt ?? (at(.generatingProof) != nil ? witnessStart : nil), at(.generatingProof)),
            Step("Zero-knowledge proof on this iPhone", "The phone proves physical consistency without disclosing the private sensor readings.", at(.generatingProof), at(.localProofVerification), showsProofPhases: true),
            Step("Local verification", "The phone checks that the proof matches the public inputs before signing a PROOF transaction.", at(.localProofVerification), at(.submittingProof)),
            Step("PROOF on XRPL", "Testnet validates the transaction carrying the proof.", at(.submittingProof), proofValidated, transaction: journal?.proof, ledger: journal?.proofLedger),
            Step("Certified", "The phone saves a photo with verification references embedded in the file.", journal?.proofValidatedAt, at(.certified)),
        ]
    }

    var body: some View {
        NavigationStack {
            ScrollViewReader { scroll in
                ScrollView {
                    let steps = timeline
                    VStack(alignment: .leading, spacing: 20) {
                        summary(steps).id("summary")
                        if let reveal { challengeCard(reveal) }
                        VStack(alignment: .leading, spacing: 0) {
                            Text("Certification timeline").font(.headline).padding(.bottom, 14)
                            ForEach(steps) { step in
                                timelineRow(step, last: step.id == steps.last?.id)
                            }
                        }
                    }
                    .frame(maxWidth: 620)
                    .padding(20)
                    .frame(maxWidth: .infinity)
                }
                .onChange(of: row?.state.isTerminal) { _, terminal in
                    if terminal == true {
                        withAnimation(reduceMotion ? nil : .easeOut(duration: 0.25)) { scroll.scrollTo("summary", anchor: .top) }
                    }
                }
            }
            .background(Color(uiColor: .systemGroupedBackground))
            .navigationTitle("Galaxy · XRPL Testnet")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button(model.isCertifying ? "Hide" : "Done") { dismiss() }
                }
            }
            .task(id: previewSource) { await loadPreview(previewSource) }
            .onChange(of: reveal != nil && preview != nil, initial: true) { _, revealed in
                withAnimation(reduceMotion ? nil : .easeOut(duration: 0.55)) { showTiles = revealed }
            }
            .onChange(of: row?.state == .certified, initial: true) { _, certified in celebrate = certified }
        }
    }

    private func summary(_ steps: [Step]) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            if let row, row.state.isTerminal {
                result(row)
            } else if model.isCertifying && row?.state == .waitingSelectionLedger {
                ledgerTicker
            } else if model.isCertifying {
                let active = steps.first { $0.start != nil && $0.end == nil }
                HStack(spacing: 10) {
                    ProgressView().tint(.cyan)
                    Text(active?.title ?? "Reading pre-capture ledger").font(.title3.bold())
                }
                Text(active?.caption ?? "The phone fixes the current validated ledger before it takes the photo.")
                    .foregroundStyle(.secondary)
                if row?.state == .generatingProof, let start = active?.start {
                    proofStatus(start: start)
                }
            } else {
                Label(row == nil ? "Capture didn’t start" : "Certification paused", systemImage: "pause.circle.fill")
                    .font(.title3.bold())
                if let message = model.message { Text(message).foregroundStyle(.secondary) }
                if row != nil {
                    Button("Resume this attempt", action: model.resume)
                        .buttonStyle(.borderedProminent).disabled(!model.idle || !model.loaded)
                }
            }
            if let retry = model.ledgerRetry {
                Label("Waiting for XRPL (retry \(retry))", systemImage: "arrow.clockwise")
                    .font(.subheadline).foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(16)
        .background(.background, in: RoundedRectangle(cornerRadius: 18))
    }

    private var ledgerTicker: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label("Waiting for the challenge ledger", systemImage: "point.3.connected.trianglepath.dotted")
                .font(.title3.bold())
            if let target = model.selectionTarget, let inclusion = journal?.captureLedger {
                HStack(spacing: 16) {
                    if let current = model.validatedIndex, target > inclusion {
                        let remaining = current < target ? target - current : 0
                        let span = target - inclusion
                        let filled = span - min(span, remaining)
                        ZStack {
                            Circle().strokeBorder(.cyan.opacity(0.18), lineWidth: 5)
                            Circle().trim(from: 0, to: Double(filled) / Double(span))
                                .stroke(.cyan, style: StrokeStyle(lineWidth: 5, lineCap: .round))
                                .rotationEffect(.degrees(-90))
                                .animation(reduceMotion ? nil : .easeOut(duration: 0.35), value: filled)
                            Text(remaining.formatted()).font(.title2.bold().monospacedDigit())
                                .contentTransition(.numericText())
                        }
                        .frame(width: 58, height: 58).accessibilityHidden(true)
                        VStack(alignment: .leading, spacing: 4) {
                            Text("Validated ledger \(current.formatted())").font(.headline.monospacedDigit())
                            Text("Challenge ledger L0+\(span): \(target.formatted())")
                            Text(remaining == 0 ? "Validated · reading its header" : "\(remaining) to go")
                                .foregroundStyle(.secondary)
                        }
                    } else {
                        ProgressView()
                        Text("Challenge ledger \(target.formatted()) · reading validated index")
                    }
                }.font(.subheadline)
            } else {
                ProgressView("Reading the selection window")
            }
            Text("The next sample is chosen by Testnet, not by the photographer.")
                .font(.subheadline).foregroundStyle(.secondary)
        }
    }

    private func challengeCard(_ reveal: ChallengeReveal) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Challenge revealed · \(reveal.tiles.count) sampled tiles", systemImage: "square.grid.3x3")
                .font(.headline)
            if let preview {
                Image(uiImage: preview)
                    .resizable().aspectRatio(contentMode: .fit)
                    .overlay {
                        Canvas { context, size in
                            // The committed raster is sensor-native; tile labels are row-major (8 × 6).
                            let w = size.width / 8, h = size.height / 6
                            for tile in reveal.tiles where tile < 48 {
                                let rect = CGRect(x: CGFloat(tile % 8) * w, y: CGFloat(tile / 8) * h, width: w, height: h)
                                context.fill(Path(rect), with: .color(.cyan.opacity(0.32)))
                                context.stroke(Path(rect.insetBy(dx: 0.8, dy: 0.8)), with: .color(.cyan), lineWidth: 1.4)
                            }
                        }.opacity(showTiles ? 1 : 0)
                    }
                    .clipShape(.rect(cornerRadius: 8))
                    .frame(maxWidth: 220)
                    .accessibilityLabel("Captured photo with \(reveal.tiles.count) sampled tiles highlighted")
            }
            Text("These regions were chosen by ledger \(reveal.ledger.formatted()) — unknown when the photo was taken.")
                .font(.subheadline).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(16)
        .background(.background, in: RoundedRectangle(cornerRadius: 18))
    }

    private func proofStatus(start: Date) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            if let count = model.pin?.constraintCount {
                Text("\(count.formatted()) circuit constraints").font(.subheadline.monospacedDigit())
            }
            TimelineView(.periodic(from: start, by: 1)) { context in
                Text("\(max(0, context.date.timeIntervalSince(start)).formatted(.number.precision(.fractionLength(0)))) s elapsed")
                    .monospacedDigit()
            }
            if let active = model.progress {
                let time = active.seconds.map { " · " + seconds($0) } ?? ""
                Text(proofPhaseTitle(active.phase) + time).foregroundStyle(.secondary)
            }
        }.font(.subheadline)
    }

    @ViewBuilder private func result(_ row: CaptureRecord) -> some View {
        if row.state == .certified {
            HStack {
                Image(systemName: "checkmark.seal.fill")
                    .symbolEffect(.bounce, value: celebrate && !reduceMotion)
                    .accessibilityHidden(true)
                Text("CERTIFIED")
            }.font(.title2.bold()).foregroundStyle(.green)
            Text("Physical consistency certified under the Galaxy policy on XRPL Testnet.")
            if let details {
                Text("Photo ID").font(.caption).foregroundStyle(.secondary)
                Text(details.photoId).font(.caption.monospaced()).textSelection(.enabled)
            }
            if let capture = journal?.capture, let url = explorerURL(for: capture) {
                Link("CAPTURE · Testnet Explorer", destination: url).font(.subheadline)
            }
            if let proof = journal?.proof, let url = explorerURL(for: proof) {
                Link("PROOF · Testnet Explorer", destination: url).font(.subheadline)
            }
            NavigationLink {
                GalleryDetail(model: model, library: library, item: .attempt(row))
            } label: {
                Label("Verify", systemImage: "checkmark.shield")
            }.buttonStyle(.borderedProminent)
            Text("Open this photo’s verification references and export the original for independent verification.")
                .font(.footnote).foregroundStyle(.secondary)
        } else {
            let inconclusive = row.state == .inconclusive
            Label(inconclusive ? "INCONCLUSIVE" : "CERTIFICATION FAILED", systemImage: "exclamationmark.triangle.fill")
                .font(.title2.bold()).foregroundStyle(inconclusive ? .yellow : .red)
            Text(inconclusive ? "This attempt did not produce a proof." : "No certified photo was released.")
            ForEach(Array(row.reasons.enumerated()), id: \.offset) { _, reason in
                let explanation = explanation(for: reason)
                VStack(alignment: .leading, spacing: 3) {
                    Text(explanation.title).font(.subheadline.weight(.semibold))
                    if let tip = explanation.tip { Text(tip).font(.footnote).foregroundStyle(.secondary) }
                }
            }
        }
    }

    private struct Step: Identifiable {
        var id: String { title }
        let title: String
        let caption: String
        let start: Date?
        let end: Date?
        let transaction: JournalTransaction?
        let ledger: UInt32?
        let showsProofPhases: Bool

        init(_ title: String, _ caption: String, _ start: Date?, _ end: Date?,
             transaction: JournalTransaction? = nil, ledger: UInt32? = nil, showsProofPhases: Bool = false) {
            self.title = title
            self.caption = caption
            self.start = start
            self.end = end
            self.transaction = transaction
            self.ledger = ledger
            self.showsProofPhases = showsProofPhases
        }
    }

    private func timelineRow(_ step: Step, last: Bool) -> some View {
        let done = step.end != nil
        let running = !done && step.start != nil && model.isCertifying && row?.state.isTerminal != true
        let stopped = !done && step.start != nil && !running
        return HStack(alignment: .top, spacing: 12) {
            VStack(spacing: 4) {
                Group {
                    if running { ProgressView().controlSize(.small).tint(.cyan) }
                    else if done { Image(systemName: "checkmark.circle.fill").foregroundStyle(.green) }
                    else if stopped { Image(systemName: "pause.circle").foregroundStyle(.yellow) }
                    else { Image(systemName: "circle").foregroundStyle(.tertiary) }
                }.frame(width: 20, height: 20)
                if !last { Rectangle().fill(.quaternary).frame(width: 1.5).frame(maxHeight: .infinity) }
            }
            .accessibilityLabel(done ? "Done" : running ? "Running" : stopped ? "Stopped" : "Pending")
            VStack(alignment: .leading, spacing: 5) {
                HStack(alignment: .firstTextBaseline) {
                    Text(step.title).font(.subheadline.weight(.semibold)).foregroundStyle(done || running ? .primary : .secondary)
                    Spacer(minLength: 4)
                    if let start = step.start, let end = step.end, end >= start {
                        Text(seconds(end.timeIntervalSince(start))).font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                    }
                }
                Text(step.caption).font(.footnote).foregroundStyle(.secondary)
                if let tx = step.transaction {
                    transactionLink(tx, ledger: step.ledger)
                }
                if step.showsProofPhases {
                    ForEach(model.proofPhases, id: \.phase) { tick in
                        HStack {
                            if tick.seconds == nil && running { ProgressView().controlSize(.mini) }
                            Text(proofPhaseTitle(tick.phase))
                            Spacer()
                            if let duration = tick.seconds { Text(seconds(duration)).monospacedDigit() }
                        }.font(.caption).foregroundStyle(.secondary)
                    }
                }
            }.padding(.bottom, last ? 0 : 14)
        }
        .fixedSize(horizontal: false, vertical: true)
    }

    private func transactionLink(_ tx: JournalTransaction, ledger: UInt32?) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(ledger.map { "Validated in ledger \($0.formatted())" } ?? "Signed · not yet validated")
                .font(.caption).foregroundStyle(.secondary)
            if let url = explorerURL(for: tx) {
                Link(destination: url) {
                    Label(tx.hash, systemImage: "arrow.up.right.square").font(.caption2.monospaced())
                }.accessibilityHint("Opens the XRPL Testnet explorer")
            }
        }
    }

    private func explorerURL(for tx: JournalTransaction) -> URL? {
        let configured = tx.hash == journal?.capture?.hash ? details?.captureTxUrl : details?.proofTxUrl
        // The export includes the pinned explorer URLs. Before export this is a
        // Testnet-only link to a signed hash, not a verification result.
        guard let url = URL(string: configured ?? "https://testnet.xrpl.org/transactions/\(tx.hash)"),
              url.scheme == "https", url.lastPathComponent.uppercased() == tx.hash else { return nil }
        return url
    }

    private func loadPreview(_ source: GalleryImageSource?) async {
        guard let source else { return }
        if preview == nil, let bytes = try? await library.thumbnail(source), !Task.isCancelled {
            preview = UIImage(data: bytes)
        }
        if source.certified, let bytes = try? await library.image(source),
           let references = try? await library.certifiedReferences(bytes), !Task.isCancelled {
            details = references
        }
    }

    private func seconds(_ value: Double) -> String {
        value > 0 && value < 0.01 ? "<0.01 s" : value.formatted(.number.precision(.fractionLength(2))) + " s"
    }

    private func proofPhaseTitle(_ name: String) -> String {
        switch name {
        case "witness": "Evaluate the circuit witness"
        case "witness_map": "Map the witness to the proof polynomial"
        case "msm_h": "Compute proof terms · H"
        case "msm_l": "Compute proof terms · L"
        case "msm_a": "Compute proof terms · A"
        case "msm_b1": "Compute proof terms · B1"
        case "msm_b2": "Compute proof terms · B2"
        case "finalize": "Assemble the proof"
        case "self_verify": "Check the generated proof"
        default: name
        }
    }

    private func explanation(for reason: String) -> (title: String, tip: String?) {
        switch reason {
        case "Capture quality predicate failed.":
            return ("The sensor readings weren’t clear enough.", "Try again in steady light with a clear view of the subject.")
        case "LiDAR geometry predicate failed.":
            return ("The sampled depth didn’t show enough consistent 3D structure.", "Frame textured objects at different depths rather than a flat surface.")
        case "RGB–depth consistency predicate failed.":
            return ("The photo’s detail didn’t agree with the sampled depth.", "Choose a well-lit scene with clear edges; avoid glass and mirrors.")
        case "Temporal consistency predicate failed.":
            return ("The sampled details didn’t agree across frames.", "Choose a still scene and move the phone gently through the shutter.")
        case "Camera motion predicate failed.":
            return ("Camera motion didn’t agree with the motion sensors or was outside the allowed range.", "Keep the phone handheld and move gently without twisting it.")
        default:
            if let issue = CaptureIssue.allCases.first(where: { $0.message == reason }) {
                switch issue {
                case .tracking, .missingFrame:
                    return ("The camera lost tracking or missed a frame.", "Wait for the camera’s tracking indicator to settle, then try again.")
                case .missingDepth, .noUsableDepth:
                    return ("Not enough usable LiDAR depth.", "Aim at solid objects at different depths within a few meters.")
                case .noMotion:
                    return ("The phone barely moved during capture.", "Hold it in your hands instead of resting it on a support.")
                case .excessRotation:
                    return ("The phone turned too far during capture.", "Move it gently without twisting it.")
                case .motionUnavailable:
                    return ("The motion readings were incomplete.", "Keep the app open and try the capture again.")
                case .imageFormat, .cameraUnavailable:
                    return ("The required camera format was unavailable.", "Return to the camera and wait for the LiDAR preview to be ready.")
                }
            }
            return (reason, nil)
        }
    }
}
