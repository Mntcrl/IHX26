import SwiftUI

@main
struct XRPL_GalaxyApp: App {
    var body: some Scene {
        WindowGroup {
            #if DEBUG
            if ProcessInfo.processInfo.arguments.contains("--app-attest-probe") {
                AppAttestProbe()
            } else {
                CaptureScreen()
            }
            #else
            CaptureScreen()
            #endif
        }
    }
}
