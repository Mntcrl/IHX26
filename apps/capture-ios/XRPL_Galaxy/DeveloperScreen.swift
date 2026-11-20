import SwiftUI

struct DeveloperScreen: View {
    @Environment(\.dismiss) private var dismiss
    let capture: Acquisition

    var body: some View {
        NavigationStack {
            Form {
                Section("Sensors") {
                    LabeledContent("Camera", value: capture.cameraStatus)
                    LabeledContent("Video format", value: capture.videoFormat)
                    LabeledContent("Depth coverage", value: capture.depthCoverage.map { $0.formatted(.percent.precision(.fractionLength(1))) } ?? "Unavailable")
                    LabeledContent("Camera speed", value: capture.speedMmPerSecond.map { String(format: "%.1f mm/s", $0) } ?? "Unavailable")
                    LabeledContent("CoreMotion", value: capture.imuStatus)
                    LabeledContent("Bluetooth", value: capture.bluetoothStatus)
                    if let clock = capture.clockSummary { Text(clock).font(.caption.monospacedDigit()) }
                }
                Section("Private calibration fixture") {
                    Text("GXRW contains unredacted camera, depth and motion data. Export is local only. Pull it only with the photographer’s consent, and never publish it.")
                    Button("Export last raw capture", action: capture.exportLastRaw)
                        .disabled(capture.lastRaw == nil)
                    if let notice = capture.notice { Text(notice).font(.caption) }
                }
            }
            .navigationTitle("Developer")
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
        }
    }
}
