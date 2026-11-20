# ADR 0008 — Build and device workflow

Status: accepted (2026-11-20).

## Context
- Mac build host: macOS 26.4, M2, 8 GB RAM, 8.8 GiB free disk (96 % used), Xcode 26.6 / iOS SDK 26.5, rustc 1.92.0 without iOS targets (recon §8).
- `xcodebuild -scheme XRPL_Galaxy` fails ("iOS 26.5 is not installed"; no eligible destinations), while `xcodebuild -target XRPL_Galaxy -sdk iphoneos` resolves settings (recon §3). The device runs iOS 27.2.1.
- Provisioning: personal team PPS57BF3X3, bundle id `utcq.XRPL-Galaxy`, profiles expire 2026-11-27 (recon §4).
- The user's Mac working tree has uncommitted changes and must not be touched (recon §1).
- Linux workstation: 8 CPUs, 13 GiB RAM, rustc 1.91.1 (recon §9).

## Decision
- Rust toolchain pinned to 1.91.1 (`rust-toolchain.toml`) with `wasm32-unknown-unknown` and `aarch64-apple-ios` targets; cross-compile Rust for iOS on Linux where possible to spare Mac disk and RAM.
- Mac builds run in `/Users/unity/Workspace/IXH26/galaxy-build/<subtree>/`, synced with `scripts/lib-mac.sh` `mac_rsync` (`rsync -az --delete`, excluding `target/`, `node_modules/`, `build/`, `DerivedData/`); DerivedData lives under the subtree's `build/`. Each agent keeps its Mac footprint below 3 GB.
- iOS builds use `xcodebuild -target … -sdk iphoneos` (no scheme/destination), install/launch with `xcrun devicectl`.
- Device access is serialized with `mkdir /tmp/galaxy-device.lock` on the Mac (≤ 15 min hold).
- Repo-local tools in `.tools/` (gitignored): circom built from source at a pinned tag, `wasm-bindgen-cli` matching the workspace `wasm-bindgen` version, ptau files.
- `make doctor` reports the state of all of the above.

## Consequences
- Profiles must be refreshed before 2026-11-27 (Xcode automatic signing, `-allowProvisioningUpdates`).
- If the iOS 26.5 platform component stays missing, only `-target` builds are possible; tests that need a scheme cannot run.
