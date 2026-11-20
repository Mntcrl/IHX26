import Foundation

/// The app's file-system writer. It never replaces a file that this app did not create:
/// `create` refuses existing paths and `replace` is used only for a capture record inside the
/// app's own per-capture UUID directory. The only deletion in the app is RawCaptureWriter
/// removing the in-progress file it has just created itself, by exact path, after a failed write.
nonisolated enum ProtectedFiles {
    /// Creates `url` (and missing parents) with complete protection, excluded from backup.
    /// An existing directory is used as is: its attributes are not changed.
    static func directory(_ url: URL) throws {
        let fm = FileManager.default
        var isDirectory: ObjCBool = false
        if fm.fileExists(atPath: url.path, isDirectory: &isDirectory) {
            guard isDirectory.boolValue else { throw CocoaError(.fileWriteFileExists) }
            return
        }
        try fm.createDirectory(at: url, withIntermediateDirectories: true,
                               attributes: [.protectionKey: FileProtectionType.complete])
        try excludeFromBackup(url)
    }

    /// Writes a new file; fails if anything already exists at `url`.
    static func create(_ bytes: Data, at url: URL) throws {
        try bytes.write(to: url, options: [.withoutOverwriting, .completeFileProtection])
        try excludeFromBackup(url)
    }

    /// Atomically replaces a file the caller owns (a capture record inside its own UUID directory).
    static func replace(_ bytes: Data, at url: URL) throws {
        try bytes.write(to: url, options: [.atomic, .completeFileProtection])
        try excludeFromBackup(url)
    }

    private static func excludeFromBackup(_ url: URL) throws {
        var item = url
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try item.setResourceValues(values)
    }
}
