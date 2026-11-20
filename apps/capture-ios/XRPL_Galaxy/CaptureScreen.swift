import ARKit
import SwiftUI

struct CaptureScreen: View {
    @Environment(\.scenePhase) private var scenePhase
    @State private var capture = Acquisition()
    @State private var showDeveloper = false

    var body: some View {
        ZStack {
            Color.black.ignoresSafeArea()
            CameraPreview(view: capture.preview).ignoresSafeArea().accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 10) {
                    Text("XRPL Galaxy").font(.headline)
                    Picker("Scene class", selection: $capture.sceneLabel) {
                        ForEach(SceneLabel.allCases) { Text($0.title).tag($0) }
                    }
                    .pickerStyle(.menu)
                    .tint(capture.sceneLabel == .unlabelled ? .orange : .white)
                    .background(.black.opacity(0.6), in: Capsule())
                    .accessibilityLabel("Scene class of the next capture")
                    Spacer()
                    Text("\(capture.attempts) attempts · \(capture.exported) saved")
                        .font(.caption.monospacedDigit())
                        .padding(6)
                        .background(.black.opacity(0.6), in: Capsule())
                    Button("Developer", systemImage: "waveform.path.ecg") { showDeveloper = true }
                        .buttonStyle(.bordered)
                }
                Spacer()
                HStack(alignment: .bottom) {
                    VStack(alignment: .leading, spacing: 5) {
                        Text("Slide the phone slowly sideways (2–8 cm/s), keep it level, press the shutter while moving.")
                            .font(.callout.weight(.semibold))
                        HStack(spacing: 6) {
                            Circle().fill(motionColor).frame(width: 10, height: 10)
                            Text(motionText).font(.caption.monospacedDigit())
                        }
                        Text(depthText).font(.caption.monospacedDigit())
                        Text(capture.cameraStatus).font(.caption)
                        if let record = capture.record {
                            Text(record.state.rawValue).font(.caption.monospaced())
                            ForEach(record.reasons, id: \.self) { Text($0).font(.caption) }
                        } else { Text("READY").font(.caption.monospaced()) }
                        if capture.record?.state == .canonicalizing {
                            Text("Raw capture saved locally. This image is not certified.").font(.caption)
                        }
                        if let notice = capture.notice { Text(notice).font(.caption) }
                    }
                    .padding(10)
                    .background(.black.opacity(0.75), in: RoundedRectangle(cornerRadius: 10))
                    Spacer(minLength: 18)
                    Button(action: capture.capture) {
                        Image(systemName: "camera.circle.fill").font(.system(size: 54))
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(!capture.canCapture)
                    .accessibilityLabel("Capture photo and depth")
                }
            }
            .padding()
        }
        .preferredColorScheme(.dark)
        .onChange(of: scenePhase, initial: true) { _, phase in
            capture.setActive(phase == .active)
        }
        .sheet(isPresented: $showDeveloper) { DeveloperScreen(capture: capture) }
    }

    /// Camera motion over the last ~250 ms (the t−/t+ spacing): the policy needs ≥ 3 mm of
    /// translation per side and small rotation; very fast motion causes occlusions and tracking loss.
    private var motionText: String {
        guard let mm = capture.baselineMillimeters, let deg = capture.rotationDegrees else {
            return "Motion: measuring…"
        }
        let base = String(format: "Last 250 ms: %.0f mm, %.1f°", mm, deg)
        switch motionGrade {
        case .good: return base + " · good"
        case .tooFast: return base + " · too fast"
        case .tooSlow: return base + " · move more"
        case .rotating: return base + " · rotate less"
        case .unknown: return base
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

    private var motionColor: Color {
        switch motionGrade {
        case .good: .green
        case .tooSlow, .rotating: .yellow
        case .tooFast: .red
        case .unknown: .gray
        }
    }

    private var depthText: String {
        guard let coverage = capture.depthCoverage else { return "Depth: unavailable" }
        return "Depth coverage (0.2–5 m): \(Int((coverage * 100).rounded())) %"
    }
}

private struct CameraPreview: UIViewRepresentable {
    let view: ARSCNView

    func makeUIView(context: Context) -> ARSCNView { view }
    func updateUIView(_ uiView: ARSCNView, context: Context) {}
}
