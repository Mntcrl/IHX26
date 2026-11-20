import Foundation

nonisolated enum ScanExport {
    // Sensor values can be NaN or infinite. Preserve their positions in arrays
    // and matrices with JSON null. The raw depth file keeps the original bits.
    private static func jsonValue(_ value: Any) -> Any {
        switch value {
        case let object as [String: Any]:
            return object.mapValues(jsonValue)
        case let array as [Any]:
            return array.map(jsonValue)
        case let number as NSNumber:
            return number.doubleValue.isFinite ? number : NSNull()
        case let string as String:
            return string
        case let data as Data:
            return ["encoding": "base64", "data": data.base64EncodedString()]
        case let date as Date:
            return ISO8601DateFormatter().string(from: date)
        default:
            return NSNull()
        }
    }

    static func encodeJSON(_ value: [String: Any]) throws -> Data {
        try JSONSerialization.data(withJSONObject: jsonValue(value), options: [.sortedKeys])
    }

    static func writeManifest(_ metadata: [String: Any], meshCount: Int,
                              meshAt: (Int) -> [String: Any], to url: URL) throws {
        try Data().write(to: url)
        let file = try FileHandle(forWritingTo: url)
        defer { try? file.close() }

        let options: JSONSerialization.WritingOptions = [.sortedKeys]
        var header = try JSONSerialization.data(withJSONObject: jsonValue(metadata), options: options)
        header.removeLast() // Close the outer object after streaming the mesh.
        try file.write(contentsOf: header)
        try file.write(contentsOf: Data(",\"reconstructedMeshAnchors\":[".utf8))
        for index in 0..<meshCount {
            try autoreleasepool {
                if index > 0 { try file.write(contentsOf: Data(",".utf8)) }
                let chunk = try JSONSerialization.data(withJSONObject: jsonValue(meshAt(index)), options: options)
                try file.write(contentsOf: chunk)
            }
        }
        try file.write(contentsOf: Data("]}".utf8))
        try file.synchronize()
    }

    static func makeArchive(files: [URL], name: String, disclosure: Data? = nil) throws -> URL {
        let manager = FileManager.default
        let work = manager.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        let source = work.appendingPathComponent(name, isDirectory: true)
        let archive = work.appendingPathComponent(name).appendingPathExtension("zip")
        try manager.createDirectory(at: source, withIntermediateDirectories: true)
        do {
            // A missing attachment fails the export instead of sending a partial ZIP.
            for file in files {
                try manager.copyItem(at: file, to: source.appendingPathComponent(file.lastPathComponent))
            }
            if let disclosure {
                try disclosure.write(to: source.appendingPathComponent("inclusion-proof.json"), options: .atomic)
            }
            var coordinationError: NSError?
            var copyError: Error?
            NSFileCoordinator().coordinate(readingItemAt: source, options: .forUploading,
                                           error: &coordinationError) { zippedURL in
                do { try manager.copyItem(at: zippedURL, to: archive) }
                catch { copyError = error }
            }
            if let coordinationError { throw coordinationError }
            if let copyError { throw copyError }
            guard manager.fileExists(atPath: archive.path) else { throw CocoaError(.fileWriteUnknown) }
            try manager.removeItem(at: source)
            return archive
        } catch {
            try? manager.removeItem(at: work)
            throw error
        }
    }

    static func removeArchive(_ archive: URL) {
        try? FileManager.default.removeItem(at: archive.deletingLastPathComponent())
    }
}
