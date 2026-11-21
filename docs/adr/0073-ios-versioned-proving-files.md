# ADR 0073 — Install iOS proving files separately by circuit ID

Status: accepted (IOS).

## Context

The v1m device prover needs a witness graph and an approximately 830 MB zkey. Apple’s [App Store build size reference](https://developer.apple.com/help/app-store-connect/reference/app-uploads/maximum-build-file-sizes/) lists an iOS app limit of 4 GB uncompressed; it is not a guarantee about Personal Team wired deployment. Bundling the pair would replicate it in each Mac build product, make every reinstall transfer it again, and increase signing and install overhead. App Store-hosted resources are not needed for the development install.

## Decision

- Bundle only the approved descriptor. Build one `libgalaxy_ffi.a` with the matched network and policy pins, and sync the Swift bindings and C module alongside it. Validate `BUILD-INFO.txt` and sync only the selected descriptor and network config, never the whole circuit output directory.
- Install the public graph/zkey with `scripts/ios.sh push-artifacts` via the Mac mirror and a device-locked, in-container `devicectl device copy to`. No runtime artifact server or app download permission. Stage each pair to a new immutable slot under `Library/Application Support/galaxy-artifacts/<circuit_id>/`, then copy a `READY` marker. An interrupted copy without the marker cannot be used; no installation overwrites a file a live prover may have mapped.
- The app selects the newest marked slot for that exact descriptor. GalaxyCore checks both SHA-256 digests, verification-key digest and public input count before enabling the certification shutter. Missing or mismatched files block certified operation, not developer-only raw acquisition.

## Consequences

The signed app retains only a small descriptor, and public proving files can persist through in-place update independently of the binary. Reinstalling the capture binary does not copy the large pair. A new circuit needs its own matching slot; never remap outstanding captures to a replacement descriptor. An artifact refresh requires a relaunch to replace mappings; failed or old device slots are not deleted by the install command. The [iOS capture runbook](../protocol/ios-capture.md) covers Mac staging, device transfer and verification.
