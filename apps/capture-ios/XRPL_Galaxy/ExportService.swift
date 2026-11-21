import Foundation
import Photos
import SwiftUI
import UniformTypeIdentifiers

/// A copy of the exact Rust-encoded file, including the XMP packet. Never decode into UIImage.
struct CertifiedFile: FileDocument {
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
            throw FlowError.paused("Photos access was denied. You can still export the original file to Files.")
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
