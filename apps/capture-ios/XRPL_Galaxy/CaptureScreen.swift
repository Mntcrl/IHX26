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
                HStack {
                    Text("XRPL Galaxy").font(.headline)
                    Spacer()
                    Button("Developer", systemImage: "waveform.path.ecg") { showDeveloper = true }
                        .buttonStyle(.bordered)
                }
                Spacer()
                HStack(alignment: .bottom) {
                    VStack(alignment: .leading, spacing: 5) {
                        Text("Move the phone slightly sideways while capturing.").font(.callout.weight(.semibold))
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
}

private struct CameraPreview: UIViewRepresentable {
    let view: ARSCNView

    func makeUIView(context: Context) -> ARSCNView { view }
    func updateUIView(_ uiView: ARSCNView, context: Context) {}
}
