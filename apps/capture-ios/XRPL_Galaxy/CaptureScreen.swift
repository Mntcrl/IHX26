import ARKit
import SwiftUI

struct CaptureScreen: View {
    @Environment(\.scenePhase) private var scenePhase
    @State private var capture = Acquisition()
    @State private var showDeveloper = false

    var body: some View {
        GeometryReader { geometry in
            let portrait = geometry.size.height > geometry.size.width
            ZStack {
                Color.black.ignoresSafeArea()
                CameraPreview(view: capture.preview).ignoresSafeArea().accessibilityHidden(true)
                OverlayCanvas(model: capture.overlay).ignoresSafeArea().accessibilityHidden(true)
                VStack(spacing: 8) {
                    topBar
                    HStack(alignment: .top) {
                        Spacer()
                        if capture.overlay.showTrail { MotionTrail(model: capture.overlay) }
                    }
                    Spacer()
                    if portrait {
                        VStack(spacing: 8) {
                            statusChips
                            toggles
                            shutter
                        }
                    } else {
                        HStack(alignment: .bottom, spacing: 12) {
                            VStack(alignment: .leading, spacing: 8) {
                                toggles
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
            capture.setActive(phase == .active)
        }
        .sheet(isPresented: $showDeveloper) { DeveloperScreen(capture: capture) }
    }

    private var topBar: some View {
        HStack(spacing: 8) {
            Picker("Scene", selection: $capture.sceneLabel) {
                ForEach(SceneLabel.allCases) { Text($0.title).tag($0) }
            }
            .pickerStyle(.menu)
            .tint(capture.sceneLabel == .unlabelled ? .orange : .white)
            .padding(.horizontal, 4)
            .background(.black.opacity(0.55), in: Capsule())
            .accessibilityLabel("Scene class of the next capture")
            Spacer()
            Chip(systemImage: "number", text: "\(capture.attempts) · \(capture.exported) saved")
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
                Text(String(format: "%.0f fps", capture.overlay.framesPerSecond))
                    .font(.caption2.monospacedDigit()).foregroundStyle(.secondary)
            }
            HStack(spacing: 6) {
                Chip(systemImage: "move.3d", text: motionText, tint: motionColor)
                if !capture.trackingNormal { Chip(systemImage: "exclamationmark.triangle", text: "tracking", tint: .yellow) }
                if let notice = capture.notice { Chip(systemImage: "xmark.octagon", text: notice, tint: .red) }
            }
        }
    }

    private var shutter: some View {
        Button(action: capture.capture) {
            ZStack {
                Circle().stroke(.white, lineWidth: 4).frame(width: 74, height: 74)
                Circle().fill(capture.canCapture ? motionColor.opacity(0.9) : .gray.opacity(0.5)).frame(width: 60, height: 60)
            }
        }
        .disabled(!capture.canCapture)
        .accessibilityLabel("Capture photo and depth")
    }

    // MARK: - Status text

    private var stateText: String {
        guard let record = capture.record else { return "READY" }
        switch record.state {
        case .canonicalizing: return "SAVED"
        case .inconclusive, .failed:
            let codes = Array(Set(record.reasons.map(CaptureIssue.short(for:)))).sorted()
            return codes.isEmpty ? record.state.rawValue : "\(record.state.rawValue) · \(codes.joined(separator: ", "))"
        default: return record.state.rawValue
        }
    }

    private var stateSymbol: String {
        switch capture.record?.state {
        case .canonicalizing: "checkmark.circle.fill"
        case .inconclusive: "questionmark.circle"
        case .failed: "xmark.circle"
        case .acquiring: "record.circle"
        default: "circle"
        }
    }

    private var stateTint: Color {
        switch capture.record?.state {
        case .canonicalizing: .green
        case .inconclusive: .yellow
        case .failed: .red
        case .acquiring: .orange
        default: .white
        }
    }

    private enum MotionGrade { case unknown, good, tooSlow, tooFast, rotating }

    private var motionGrade: MotionGrade {
        guard let mm = capture.baselineMillimeters, let deg = capture.rotationDegrees else { return .unknown }
        if mm > 40 { return .tooFast }
        if deg > 4 { return .rotating }
        if mm < 4 { return .tooSlow }
        return .good
    }

    private var motionText: String {
        guard let mm = capture.baselineMillimeters, let deg = capture.rotationDegrees else { return "—" }
        let value = String(format: "%.0f mm %.1f°", mm, deg)
        switch motionGrade {
        case .tooFast: return value + " fast"
        case .tooSlow: return value + " slow"
        case .rotating: return value + " turn"
        case .good, .unknown: return value
        }
    }

    private var motionColor: Color {
        switch motionGrade {
        case .good: .green
        case .tooSlow, .rotating: .yellow
        case .tooFast: .red
        case .unknown: .gray
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
