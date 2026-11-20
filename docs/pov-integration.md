# POV committee integration — hooks for the XRPL_Galaxy app

The POV client lives in six new files under `XRPL_Galaxy/` (synchronized folder, compiled automatically):
`POVModels.swift`, `POVCrypto.swift`, `POVDepth.swift`, `POVClient.swift`, `POVModel.swift`, `POVViews.swift`.
They reference no type from `ContentView.swift` and compile on their own. They are owned by the POV agents;
please do not edit them (ask instead). Everything below is applied by the app owner (TM) to the existing files.

What the hooks do: at the shutter the app takes the latest validated XRPL ledger it prefetched (T0, ≤ 20 s old);
after `saveCapture` has written the scan, `POVModel.submit` reads the saved `.jpg`, `.fdepth` and `.json`
(confidence map), signs a manifest with a Secure Enclave P-256 key (bound to T0, the JPEG/depth hashes and the
`proof.json` Merkle root), Shamir-splits the LiDAR depth into 3 shares, HPKE-seals one share per committee node
and posts them to the coordinator. The verdict appears in a sheet (PASS + XRPL anchor link, or REJECT + reason).
App Attest (`captureSession.submit`) and `saveToPhotos` stay untouched; leave `AMBIENT_BACKEND_URL` empty.

## H1 — `ContentView`: model and sheets

In `struct ContentView`, next to `@StateObject private var model = CameraModel()`:

```swift
@StateObject private var pov = POVModel()
```

On the root view of `body` (the `GeometryReader`, after the existing `.confirmationDialog("Camera settings", …) { … }`):

```swift
.task { await pov.start() }
.sheet(isPresented: $pov.showSettings) { POVSettingsSheet(pov: pov) }
.sheet(item: $pov.result) { POVResultSheet(pov: pov, attempt: $0) }
```

## H2 — header: status pill

In `private var header`, directly below the existing `statusPill` (same `VStack(alignment: .leading, spacing: 4)`):

```swift
POVStatusPill(pov: pov)
```

## H3 — "Camera settings" dialog: verification settings

In the `.confirmationDialog("Camera settings", …)` button list, before `Button("Cancel", role: .cancel) {}`:

```swift
Button("Verification settings") { pov.showSettings = true }
```

## H4 — shutter: T0 before the frame, submit after `saveCapture`

1. Shutter button in `bottomControls`: `Button { model.capture() } label: {` becomes
   `Button { model.capture(pov: pov) } label: {`.
2. In `CameraModel`, `func capture() {` becomes `func capture(pov: POVModel) {`.
3. Between `guard !isCapturing else { return }` and the guard that reads the frame
   (`guard let session, let frame = session.currentFrame, let viewport else {`), insert:

   ```swift
   let t0 = pov.freshT0() // T0 must be known before the shutter frame is read
   ```

4. Inside the `Task { … }` of `capture`, after
   `let (capture, root) = try await Task.detached(priority: .userInitiated) { try Self.saveCapture(…) }.value`
   and the existing lines `captures.insert(capture, at: 0)`, `isCapturing = false`, `announce("Saved to Scans")`, insert:

   ```swift
   if let t0, let fdepth = capture.depthURL, let json = capture.jsonURL {
       Task { await pov.submit(jpegURL: capture.url, fdepthURL: fdepth, jsonURL: json, evidenceRoot: root, t0: t0) }
   } else {
       announce("Saved locally (verification offline)")
   }
   ```

   Everything else (`photoTask`/`cameraTask`/`depthTask`, `captureSession.submit(root:window:)`, `saveToPhotos`,
   the `catch` branch) stays unchanged.

## H5 — Info.plist keys

`AmbientConfiguration.plist`, inside the top-level `<dict>`:

```xml
<key>POVCoordinatorURL</key>
<string>$(POV_COORDINATOR_URL)</string>
```

`XRPL_Galaxy.xcodeproj/project.pbxproj`, in both target build configurations (Debug and Release), next to
`INFOPLIST_KEY_NSCameraUsageDescription`:

```
INFOPLIST_KEY_NSLocalNetworkUsageDescription = "Sends encrypted proof shares to the verification committee on the local network.";
```

If the Local Network prompt still does not appear (LAN requests fail with `-1009`), add the same text directly to
`AmbientConfiguration.plist` as `<key>NSLocalNetworkUsageDescription</key>`.

## H6 — Makefile

Next to the other `?=` variables:

```make
POV_COORDINATOR_URL ?= http://10.164.228.104:8088
```

and pass it to `xcodebuild` in the `build` target (next to `AMBIENT_POST_WINDOW_SECONDS=…`):

```make
		POV_COORDINATOR_URL="$(POV_COORDINATOR_URL)" \
```

## TM acceptance

1. Unsigned compile check prints `** BUILD SUCCEEDED **`:
   `xcodebuild -target XRPL_Galaxy -sdk iphoneos -configuration Debug CODE_SIGNING_ALLOWED=NO build 2>&1 | grep -E "error:|BUILD (SUCCEEDED|FAILED)"`
2. `make run POV_COORDINATOR_URL=http://10.164.228.104:8088` installs and launches the app on the iPhone.
3. Camera settings → "Verification settings" shows a 66-hex-character device public key starting with `02` or `03`
   (Copy puts it on the clipboard). Until the committee coordinator runs on the laptop, the POV status pill shows a
   connection error; that is expected.
