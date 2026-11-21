import Foundation
import Photos
import SwiftUI
import UniformTypeIdentifiers

extension CaptureImageFormat {
    var contentType: UTType { self == .jpeg ? .jpeg : .png }
}

/// Exact exported bytes, including any XMP packet produced by GalaxyCore.
struct GalleryImageFile: FileDocument {
    static let readableContentTypes: [UTType] = [.png, .jpeg]
    private let bytes: Data

    init(url: URL) throws { bytes = try Data(contentsOf: url, options: .mappedIfSafe) }

    init(configuration: ReadConfiguration) throws {
        guard let bytes = configuration.file.regularFileContents else { throw CocoaError(.fileReadCorruptFile) }
        self.bytes = bytes
    }

    func fileWrapper(configuration: WriteConfiguration) throws -> FileWrapper { FileWrapper(regularFileWithContents: bytes) }
}

enum ExportService {
    static func saveToPhotos(_ file: URL) async throws {
        let authorization = await PHPhotoLibrary.requestAuthorization(for: .addOnly)
        guard authorization == .authorized || authorization == .limited else {
            throw FlowError.paused("Photos access was denied. You can still save the photo to Files.")
        }
        let changes: @Sendable () -> Void = {
            let options = PHAssetResourceCreationOptions()
            options.shouldMoveFile = false
            options.originalFilename = file.lastPathComponent
            PHAssetCreationRequest.forAsset().addResource(with: .photo, fileURL: file, options: options)
        }
        try await PHPhotoLibrary.shared().performChanges(changes)
    }
}
