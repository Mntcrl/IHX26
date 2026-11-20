# ADR 0071 — Aqua-session build runner for signed iOS builds

Status: accepted (2026-11-20).

## Context
- `xcodebuild` over SSH compiles the capture app but code signing fails with `errSecInternalComponent`: the login keychain is locked in the SSH security session, and the Xcode account is not readable there. Unlocking requires the user's password, which agents must not handle.
- The Mac user (`unity`) is logged in at the console; jobs started by launchd in that user's Aqua session see the unlocked keychain.

## Decision
- `apps/capture-ios/tools/mac-build-runner.sh` (synced to the Mac mirror) installs on first use a user LaunchAgent `org.galaxy.buildrunner` (`~/Library/LaunchAgents/org.galaxy.buildrunner.plist`, `LimitLoadToSessionType = Aqua`, `QueueDirectories` = `<mirror>/.galaxy-aqua-build/queue`), loaded with `launchctl bootstrap gui/<uid>` only when `/dev/console` is owned by the build user.
- A request file contains only a mode name. The worker accepts a fixed allowlist — `capture-off`, `capture-development`, `capture-production`, `capture-compile`, `proverbench`, `proverbench-fallback` — each mapped to one fixed `xcodebuild -target … -sdk iphoneos -allowProvisioningUpdates` invocation inside the mirror with build products under the subtree's `build/`. No request content is ever executed or interpolated.
- The worker writes `build.log` and an `exit` file per job (mode 0700 directory); `submit` waits up to ~9 minutes, prints the log and returns the real exit status. The runner refuses to run in the user's own workspace, needs ≥ 3 GiB free disk, verifies the product with `codesign --verify`, and reports `df -h ~` before and after.
- `scripts/ios.sh build|install|run` submit to the runner by default (`IOS_SSH_BUILD=1` builds directly over SSH); `apps/capture-ios/tools/mac-gui-build.sh <mode>` is the build-only entry point used by MobileProver. Device operations stay in `scripts/ios.sh` under `/tmp/galaxy-device.lock`; the runner never touches the device.
- `proverbench-fallback` (added by MobileProver with the orchestrator's approval) signs the benchmark app with the capture app's bundle id because the personal team cannot provision a new bundle id without an Xcode account; it replaces the capture app binary on the phone during benchmark windows (the container is kept; spec §19 device data safety applies).

## Consequences
- Signed builds work without sharing credentials (measured: capture-off signed build ≈11–15 s incremental; App Attest probe and captures ran from these builds).
- The LaunchAgent persists in the user's account until removed with `launchctl bootout gui/$(id -u)/org.galaxy.buildrunner` and deleting the plist; it only runs the fixed script in the mirror.
- Builds need a logged-in console session; with nobody logged in, `submit` fails with "No Aqua console session" (exit 5).
- New bundle ids or capabilities still require an Xcode account in the GUI (observed: "No Accounts", exit 65).
