import Foundation

/// The app's file-system writer. `create` refuses existing files; `replace` publishes
/// an atomic checkpoint only at an exact, app-owned path.
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

    /// Atomically replaces a checkpoint at a path that belongs to the caller.
    static func replace(_ bytes: Data, at url: URL) throws {
        try bytes.write(to: url, options: [.atomic, .completeFileProtection])
        try excludeFromBackup(url)
    }

    static func excludeFromBackup(_ url: URL) throws {
        var item = url
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try item.setResourceValues(values)
    }
}
