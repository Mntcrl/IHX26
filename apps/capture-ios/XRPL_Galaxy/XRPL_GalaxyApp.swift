import Foundation
import SwiftUI

@main
struct XRPL_GalaxyApp: App {
    @State private var certification = CertificationController()

    var body: some Scene {
        WindowGroup {
            #if DEBUG
            if ProcessInfo.processInfo.arguments.contains("--app-attest-probe") {
                AppAttestProbe()
            } else {
                CaptureScreen(model: certification)
            }
            #else
            CaptureScreen(model: certification)
            #endif
        }
    }
}
