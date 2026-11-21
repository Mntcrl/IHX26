import SwiftUI
import UIKit

struct GalleryScreen: View {
    @Environment(\.dismiss) private var dismiss
    let model: CertificationController
    @State private var library = GalleryLibrary()
    @State private var calibration: [CalibrationPhoto] = []
    @State private var loading = true
    @State private var loadFailed = false

    private let columns = [GridItem(.adaptive(minimum: 145), spacing: 12, alignment: .top)]

    var body: some View {
        NavigationStack {
            ScrollView {
                let attempts = model.records.compactMap { row in model.galleryImage(row).map { (row, $0) } }
                LazyVStack(alignment: .leading, spacing: 20) {
                    Text("Certification attempts").font(.headline).accessibilityAddTraits(.isHeader)
                    if attempts.isEmpty {
                        Text("No certification photos yet.").foregroundStyle(.secondary)
                    } else {
                        LazyVGrid(columns: columns, spacing: 16) {
                            ForEach(attempts, id: \.0.id) { row, source in
                                NavigationLink {
                                    GalleryDetail(model: model, library: library, item: .attempt(row))
                                } label: {
                                    GalleryTile(library: library, source: source, state: row.state,
                                                date: galleryDate(row.claimedTimeMs), subtitle: row.reasons.first)
                                }
                                .buttonStyle(.plain)
                            }
                        }
                    }
                    Text("Calibration captures").font(.headline).accessibilityAddTraits(.isHeader)
                    if loading {
                        ProgressView("Loading calibration captures")
                    } else if loadFailed {
                        Text("Could not read the protected calibration directory. Unlock the phone and reopen Gallery.")
                            .foregroundStyle(.secondary)
                    } else if calibration.isEmpty {
                        Text("No saved calibration captures.").foregroundStyle(.secondary)
                    } else {
                        Text("\(calibration.count) photos · not certified").font(.caption).foregroundStyle(.secondary)
                        LazyVGrid(columns: columns, spacing: 16) {
                            ForEach(calibration) { photo in
                                let item = GalleryItem.calibration(photo)
                                NavigationLink {
                                    GalleryDetail(model: model, library: library, item: item)
                                } label: {
                                    GalleryTile(library: library, source: photo.imageSource, state: nil,
                                                date: galleryDate(photo.claimedTimeMs), subtitle: photo.label.title)
                                }
                                .buttonStyle(.plain)
                            }
                        }
                    }
                }
                .padding()
            }
            .navigationTitle("Gallery")
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
        }
        .task {
            defer { loading = false }
            do { calibration = try await library.calibrations() }
            catch { loadFailed = true }
        }
    }
}

enum GalleryItem {
    case attempt(CaptureRecord)
    case calibration(CalibrationPhoto)
}

extension CalibrationPhoto {
    var imageSource: GalleryImageSource { .init(id: id, kind: .calibration, url: raw, format: .jpeg) }
}

func galleryDate(_ milliseconds: UInt64) -> Date {
    Date(timeIntervalSince1970: Double(milliseconds) / 1_000)
}

struct GalleryBadge: View {
    let state: CaptureState?

    private var style: (title: String, icon: String, tint: Color) {
        switch state {
        case .certified: ("Certified", "checkmark.shield", .green)
        case .inconclusive: ("Inconclusive", "questionmark.circle", .yellow)
        case .failed: ("Failed", "exclamationmark.circle", .red)
        case nil: ("Not certified", "shield.slash", .secondary)
        default: ("In progress", "hourglass", .blue)
        }
    }

    var body: some View {
        Label(style.title, systemImage: style.icon)
            .font(.caption.weight(.semibold))
            .foregroundStyle(style.tint)
    }
}

private struct GalleryTile: View {
    let library: GalleryLibrary
    let source: GalleryImageSource
    let state: CaptureState?
    let date: Date
    let subtitle: String?
    @State private var preview: UIImage?
    @State private var unavailable = false

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Color.secondary.opacity(0.16)
                .aspectRatio(4.0 / 3.0, contentMode: .fit)
                .overlay {
                    if let preview {
                        Image(uiImage: preview).resizable().scaledToFill()
                    } else if unavailable {
                        Image(systemName: "photo.badge.exclamationmark").foregroundStyle(.secondary)
                    } else { ProgressView() }
                }
                .clipShape(.rect(cornerRadius: 9))
                .accessibilityHidden(true)
            GalleryBadge(state: state)
            Text(date.formatted(date: .abbreviated, time: .shortened))
                .font(.caption).foregroundStyle(.secondary)
            if let subtitle {
                Text(subtitle).font(.caption).lineLimit(2).foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .task(id: source) {
            preview = nil
            unavailable = false
            do {
                let bytes = try await library.thumbnail(source)
                try Task.checkCancellation()
                preview = UIImage(data: bytes)
                unavailable = preview == nil
            } catch is CancellationError {
            } catch { unavailable = true }
        }
    }
}
