import Foundation
import ImageIO
import UniformTypeIdentifiers

struct CalibrationPhoto: Sendable, Identifiable {
    let id: UUID
    let label: SceneLabel
    let claimedTimeMs: UInt64
    let raw: URL

    private struct Sidecar: Decodable {
        let format: String
        let captureId: UUID
        let sceneClass: SceneLabel
        let claimedTimeMs: UInt64

        enum CodingKeys: String, CodingKey {
            case format
            case captureId = "capture_id"
            case sceneClass = "scene_class"
            case claimedTimeMs = "claimed_time_ms"
        }
    }

    static func read(raw: URL, id: UUID) throws -> Self {
        let label = raw.deletingPathExtension().appendingPathExtension("label.json")
        let info = try label.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey])
        guard info.isRegularFile == true, let size = info.fileSize, (1...65_536).contains(size) else {
            throw CocoaError(.fileReadCorruptFile)
        }
        let sidecar = try JSONDecoder().decode(Sidecar.self, from: Data(contentsOf: label))
        guard sidecar.format == "galaxy-capture-label/v1", sidecar.captureId == id else {
            throw CocoaError(.fileReadCorruptFile)
        }
        return Self(id: id, label: sidecar.sceneClass, claimedTimeMs: sidecar.claimedTimeMs, raw: raw)
    }
}

struct GalleryImageSource: Equatable, Sendable {
    enum Kind: String, Sendable {
        case certified, attempt, calibration
    }

    let id: UUID
    let kind: Kind
    let url: URL
    let format: CaptureImageFormat

    var certified: Bool { kind == .certified }
}

/// Performs local disk IO, canonicalization and thumbnail rendering off the main actor.
actor GalleryLibrary {
    func calibrations() throws -> [CalibrationPhoto] {
        let documents = try FileManager.default.url(for: .documentDirectory, in: .userDomainMask,
                                                   appropriateFor: nil, create: false)
        let directory = documents.appending(path: "galaxy-fixtures", directoryHint: .isDirectory)
        let files: [URL]
        do {
            files = try FileManager.default.contentsOfDirectory(at: directory,
                                                                includingPropertiesForKeys: [.isRegularFileKey],
                                                                options: [.skipsHiddenFiles])
        } catch CocoaError.fileReadNoSuchFile {
            return []
        }
        return files.compactMap { file in
            guard file.pathExtension == "gxrw", let id = UUID(uuidString: file.deletingPathExtension().lastPathComponent),
                  (try? file.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile) == true else { return nil }
            return try? CalibrationPhoto.read(raw: file, id: id)
        }.sorted { ($0.claimedTimeMs, $0.id.uuidString) > ($1.claimedTimeMs, $1.id.uuidString) }
    }

    func image(_ source: GalleryImageSource) throws -> Data {
        try Task.checkCancellation()
        let bytes = try Data(contentsOf: source.url, options: .mappedIfSafe)
        if source.certified { return bytes }
        return try canonicalImageFromGxrw(raw: bytes, profile: source.format.profile)
    }

    func certifiedReferences(_ bytes: Data) throws -> CertifiedPhotoDetails {
        try Task.checkCancellation()
        return try certifiedPhotoDetails(file: bytes)
    }

    func exportCertified(_ source: GalleryImageSource, as format: CaptureImageFormat, info: ExportInfo) throws -> URL {
        guard source.certified, format == .png || source.format == .jpeg else { throw GalaxyError.Image }
        let packaged = try certifiedFileWithInfo(file: image(source), info: info)
        let bytes = format == source.format ? packaged : try losslessPngFromCertifiedJpeg(file: packaged)
        return try stage(bytes, source: source, format: format)
    }

    func exportPlainJPEG(_ source: GalleryImageSource) throws -> URL {
        guard !source.certified else { throw GalaxyError.Image }
        let plain = GalleryImageSource(id: source.id, kind: source.kind, url: source.url, format: .jpeg)
        return try stage(image(plain), source: source, format: .jpeg)
    }

    func thumbnail(_ source: GalleryImageSource) throws -> Data {
        try Task.checkCancellation()
        let name = "\(source.kind.rawValue)-\(source.id.uuidString).jpg"
        let cache = try cacheDirectory("GalaxyThumbnails-v1")
        let file = cache.appending(path: name)
        if let cached = try? Data(contentsOf: file), !cached.isEmpty { return cached }
        let bytes = try image(source)
        try Task.checkCancellation()
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceThumbnailMaxPixelSize: 384,
            kCGImageSourceShouldCacheImmediately: true,
        ]
        guard let input = CGImageSourceCreateWithData(bytes as CFData, nil),
              let thumbnail = CGImageSourceCreateThumbnailAtIndex(input, 0, options as CFDictionary),
              let result = CFDataCreateMutable(nil, 0),
              let output = CGImageDestinationCreateWithData(result, UTType.jpeg.identifier as CFString, 1, nil) else {
            throw CocoaError(.fileReadCorruptFile)
        }
        CGImageDestinationAddImage(output, thumbnail, nil)
        guard CGImageDestinationFinalize(output) else { throw CocoaError(.fileWriteUnknown) }
        let encoded = result as Data
        try ProtectedFiles.replace(encoded, at: file)
        return encoded
    }

    private func stage(_ bytes: Data, source: GalleryImageSource, format: CaptureImageFormat) throws -> URL {
        try Task.checkCancellation()
        let directory = try cacheDirectory("GalaxyExports")
            .appending(path: UUID().uuidString, directoryHint: .isDirectory)
        try ProtectedFiles.directory(directory)
        let prefix = source.certified ? "Galaxy" : "Not-Certified"
        let file = directory.appending(path: "\(prefix)-\(source.id.uuidString).\(format.fileExtension)")
        try ProtectedFiles.create(bytes, at: file)
        return file
    }

    private func cacheDirectory(_ name: String) throws -> URL {
        let location = try FileManager.default.url(for: .cachesDirectory, in: .userDomainMask,
                                                  appropriateFor: nil, create: true)
            .appending(path: name, directoryHint: .isDirectory)
        try ProtectedFiles.directory(location)
        return location
    }
}
