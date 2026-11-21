import SwiftUI
import UIKit

struct DeveloperScreen: View {
    @Environment(\.dismiss) private var dismiss
    let capture: Acquisition

    #if DEBUG
    @Environment(\.scenePhase) private var scenePhase
    @State private var provingHost = ProverHost()
    @State private var provingTask: Task<Void, Never>?
    @State private var cancellation: ProvingCancellation?
    @State private var provingStatus = "Not run"
    private var isTesting: Bool { provingTask != nil }
    #else
    private var isTesting: Bool { false }
    #endif

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
                Section("Calibration mode") {
                    @Bindable var settings = capture
                    Toggle("Enable private calibration captures and overlays", isOn: $settings.calibrationMode)
                        .disabled(capture.record?.state == .acquiring || capture.writing)
                    if capture.isCalibration {
                        Toggle("Automatically export each fixture and label", isOn: $settings.autoExportFixtures)
                    }
                    Text("Calibration captures are not registered or certified.")
                }
                Section("Private calibration fixture") {
                    Text("GXRW contains unredacted camera, depth and motion data. Export is local only. Pull it only with the photographer’s consent, and never publish it.")
                    Button("Export last raw capture", action: capture.exportLastRaw)
                        .disabled(capture.lastRaw == nil)
                    if let notice = capture.notice { Text(notice).font(.caption) }
                }
                #if DEBUG
                Section("Proving self-test") {
                    Text("Offline development diagnostic using a private calibration capture and fixed local ledger values. No transaction is signed or submitted.")
                    if isTesting {
                        ProgressView(provingStatus)
                        Button("Cancel self-test", role: .cancel) { cancelSelfTest() }
                    } else {
                        Button("Run proving self-test", action: startSelfTest)
                            .disabled(!capture.canCapture)
                        Text(provingStatus).font(.caption)
                    }
                }
                #endif
            }
            .navigationTitle("Developer")
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }.disabled(isTesting)
                }
            }
            .interactiveDismissDisabled(isTesting)
            #if DEBUG
            .onChange(of: scenePhase) { _, phase in
                if phase != .active { cancelSelfTest() }
            }
            #endif
        }
    }

    #if DEBUG
    private func startSelfTest() {
        guard !isTesting, scenePhase == .active, capture.canCapture else { return }
        let token = ProvingCancellation()
        let wasKeepingAwake = UIApplication.shared.isIdleTimerDisabled
        UIApplication.shared.isIdleTimerDisabled = true
        capture.setActive(false)
        cancellation = token
        provingStatus = "Starting local proof"
        provingTask = Task {
            defer {
                provingTask = nil
                cancellation = nil
                UIApplication.shared.isIdleTimerDisabled = wasKeepingAwake
                if scenePhase == .active { capture.setActive(true) }
            }
            do {
                let result = try await provingHost.selfTest(cancellation: token) { tick in
                    guard tick.seconds == nil else { return }
                    Task { @MainActor in
                        guard cancellation === token, provingTask?.isCancelled == false else { return }
                        provingStatus = tick.phase.replacingOccurrences(of: "_", with: " ").capitalized
                    }
                }
                try Task.checkCancellation()
                provingStatus = String(format: "Verified locally · %.2f s witness · %.2f s prove",
                                       result.witnessSeconds, result.proveSeconds)
            } catch {
                provingStatus = Task.isCancelled ? "Cancelled" : FlowError.message(error)
            }
        }
    }

    private func cancelSelfTest() {
        guard let provingTask, !provingTask.isCancelled else { return }
        provingStatus = "Cancelling"
        provingTask.cancel()
        cancellation?.cancel()
    }
    #endif
}
