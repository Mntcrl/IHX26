import ARKit
import SwiftUI

struct CaptureScreen: View {
    @Environment(\.scenePhase) private var scenePhase
    @State private var capture = Acquisition()
    let model: CertificationController
    @State private var showDeveloper = false
    @State private var showCertification = false

    private var canShutter: Bool {
        capture.canCapture && (capture.isCalibration ? model.idle : model.readyToCapture)
    }

    var body: some View {
        GeometryReader { geometry in
            let portrait = geometry.size.height > geometry.size.width
            ZStack {
                Color.black.ignoresSafeArea()
                CameraPreview(view: capture.preview).ignoresSafeArea().accessibilityHidden(true)
                if capture.isCalibration {
                    OverlayCanvas(model: capture.overlay).ignoresSafeArea().accessibilityHidden(true)
                }
                VStack(spacing: 8) {
                    topBar
                    HStack(alignment: .top) {
                        Spacer()
                        if capture.isCalibration && capture.overlay.showTrail { MotionTrail(model: capture.overlay) }
                    }
                    Spacer()
                    if portrait {
                        VStack(spacing: 8) {
                            statusChips
                            if capture.isCalibration { toggles }
                            shutter
                        }
                    } else {
                        HStack(alignment: .bottom, spacing: 12) {
                            VStack(alignment: .leading, spacing: 8) {
                                if capture.isCalibration { toggles }
                                statusChips
                            }
                            Spacer()
                            shutter
                        }
                    }
                }
                .padding(12)
            }
        }
        .preferredColorScheme(.dark)
        .onChange(of: scenePhase, initial: true) { _, phase in
            capture.onFinished = { [weak model] row in model?.cameraFinished(row) }
            capture.onStorageFailure = { [weak model] row in model?.cameraStorageFailed(row) }
            if !capture.isStaticDiagnostic { model.setActive(phase == .active) }
            capture.setActive(phase == .active)
        }
        .sheet(isPresented: $showDeveloper) { DeveloperScreen(capture: capture) }
        .sheet(isPresented: $showCertification) { CertificationScreen(model: model) }
    }

    private var topBar: some View {
        HStack(spacing: 8) {
            if capture.isCalibration {
                Picker("Scene", selection: $capture.sceneLabel) {
                    ForEach(SceneLabel.allCases) { Text($0.title).tag($0) }
                }
                .pickerStyle(.menu)
                .tint(capture.sceneLabel == .unlabelled ? .orange : .white)
                .padding(.horizontal, 4)
                .background(.black.opacity(0.55), in: Capsule())
                .accessibilityLabel("Scene class of the next capture")
            } else {
                Button { showCertification = true } label: {
                    Chip(systemImage: "checkmark.shield", text: model.readyToCapture ? "Galaxy · Testnet" : "Certification & setup")
                }
            }
            Spacer()
            if capture.isCalibration {
                Chip(systemImage: "number", text: "\(capture.attempts) · \(capture.exported) saved")
            }
            Button { showDeveloper = true } label: {
                Image(systemName: "slider.horizontal.3").font(.body.weight(.semibold)).padding(8)
            }
            .background(.black.opacity(0.55), in: Circle())
            .accessibilityLabel("Developer")
        }
    }

    private var toggles: some View {
        @Bindable var overlay = capture.overlay
        return HStack(spacing: 6) {
            OverlayToggle(systemImage: "square.grid.3x3", label: "Tile grid", isOn: $overlay.showGrid)
            OverlayToggle(systemImage: "circle.grid.3x3.fill", label: "LiDAR dots", isOn: $overlay.showDots)
            OverlayToggle(systemImage: "rays", label: "Rays", isOn: $overlay.showRays)
            OverlayToggle(systemImage: "cube.transparent", label: "Depth map", isOn: $overlay.showDepth)
            OverlayToggle(systemImage: "level", label: "Level", isOn: $overlay.showLevel)
            OverlayToggle(systemImage: "scribble.variable", label: "Motion trail", isOn: $overlay.showTrail)
            OverlayToggle(systemImage: "sparkles", label: "AR feature points", isOn: $overlay.showFeatures)
        }
    }

    private var statusChips: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Chip(systemImage: stateSymbol, text: stateText, tint: stateTint)
                Chip(systemImage: "dot.radiowaves.right", text: depthText)
                if capture.isCalibration {
                    Text(String(format: "%.0f fps", capture.overlay.framesPerSecond))
                        .font(.caption2.monospacedDigit()).foregroundStyle(.secondary)
                } else if let label = model.busy {
                    Chip(systemImage: "hourglass", text: label)
                }
            }
            HStack(spacing: 6) {
                Chip(systemImage: "move.3d", text: motionText, tint: motionColor)
                if !capture.trackingNormal { Chip(systemImage: "exclamationmark.triangle", text: "tracking", tint: .yellow) }
                if let notice = capture.notice { Chip(systemImage: "xmark.octagon", text: notice, tint: .red) }
            }
        }
    }

    private var shutter: some View {
        Button {
            if capture.isCalibration { capture.capture() }
            else { model.shutter(capture) }
        } label: {
            ZStack {
                Circle().stroke(.white, lineWidth: 4).frame(width: 74, height: 74)
                Circle().fill(canShutter ? Color.white.opacity(0.92) : .gray.opacity(0.5)).frame(width: 60, height: 60)
            }
        }
        .disabled(!canShutter)
        .accessibilityLabel("Capture photo and depth")
        .accessibilityHint(capture.isCalibration ? "Private calibration only" : "Publishes to XRPL Testnet if the capture is eligible")
    }

    // MARK: - Status text

    private var displayedRecord: CaptureRecord? {
        capture.isCalibration ? capture.record : model.pending ?? model.lastAttempt
    }

    private var stateText: String {
        guard let record = displayedRecord else { return "READY" }
        switch record.state {
        case .canonicalizing where capture.isCalibration: return "SAVED · CALIBRATION"
        case .inconclusive, .failed:
            if !capture.isCalibration { return record.state.rawValue }
            let codes = Array(Set(record.reasons.map(CaptureIssue.short(for:)))).sorted()
            return codes.isEmpty ? record.state.rawValue : "\(record.state.rawValue) · \(codes.joined(separator: ", "))"
        default: return record.state.rawValue
        }
    }

    private var stateSymbol: String {
        switch displayedRecord?.state {
        case .canonicalizing where capture.isCalibration: "checkmark.circle.fill"
        case .certified: "checkmark.circle.fill"
        case .inconclusive: "questionmark.circle"
        case .failed: "xmark.circle"
        case .acquiring: "record.circle"
        default: "circle"
        }
    }

    private var stateTint: Color {
        switch displayedRecord?.state {
        case .canonicalizing where capture.isCalibration: .green
        case .certified: .green
        case .inconclusive: .yellow
        case .failed: .red
        case .acquiring: .orange
        default: .white
        }
    }

    private enum MotionGrade { case unknown, steady, resting, tooFast, rotating }

    /// Normal hand wobble is enough (policy floor 0.5 mm per side within 0.1–0.5 s, roughly
    /// ≥ 1 mm/s); only a phone resting on a support is flagged. Very fast motion breaks tracking.
    private var motionGrade: MotionGrade {
        guard let speed = capture.speedMmPerSecond, let turn = capture.rotationDegPerSecond else { return .unknown }
        if speed > 150 { return .tooFast }
        if turn > 30 { return .rotating }
        if speed < 1 { return .resting }
        return .steady
    }

    private var motionText: String {
        guard let speed = capture.speedMmPerSecond, let turn = capture.rotationDegPerSecond else { return "—" }
        let value = String(format: "%.1f mm/s %.0f°/s", speed, turn)
        return motionGrade == .resting ? value + " still" : value
    }

    private var motionColor: Color {
        switch motionGrade {
        case .steady, .unknown: .white
        case .resting, .rotating: .yellow
        case .tooFast: .red
        }
    }

    private var depthText: String {
        guard let coverage = capture.depthCoverage else { return "—" }
        return "\(Int((coverage * 100).rounded()))%"
    }
}

private struct Chip: View {
    let systemImage: String
    let text: String
    var tint: Color = .white

    var body: some View {
        Label(text, systemImage: systemImage)
            .font(.caption.monospacedDigit().weight(.medium))
            .foregroundStyle(tint)
            .lineLimit(1)
            .padding(.horizontal, 8)
            .padding(.vertical, 5)
            .background(.black.opacity(0.6), in: Capsule())
    }
}

private struct OverlayToggle: View {
    let systemImage: String
    let label: String
    @Binding var isOn: Bool

    var body: some View {
        Button { isOn.toggle() } label: {
            Image(systemName: systemImage)
                .font(.system(size: 15, weight: .semibold))
                .frame(width: 34, height: 34)
                .foregroundStyle(isOn ? .black : .white)
                .background(isOn ? Color.cyan : Color.black.opacity(0.55), in: Circle())
        }
        .accessibilityLabel(label)
        .accessibilityValue(isOn ? "on" : "off")
    }
}

private struct CameraPreview: UIViewRepresentable {
    let view: ARSCNView

    func makeUIView(context: Context) -> ARSCNView { view }
    func updateUIView(_ uiView: ARSCNView, context: Context) {}
}
