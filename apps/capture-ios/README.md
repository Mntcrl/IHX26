# XRPL Galaxy

SwiftUI capture app for the ARKit sceneDepth, nearby camera frames, CoreMotion and optional Bluetooth aggregate used by XRPL Galaxy.

- `make ios-run` from the repo root builds (in the Mac's Aqua session, see `tools/` and ADR 0071), installs and launches the app on the configured iPhone.
- See [iOS capture workflow](../../docs/protocol/ios-capture.md) for the private GXRW handoff, recovery, diagnostics and fixture export.

This version freezes raw capture evidence only. It does not produce a certificate, XRPL transaction or proof.
