# ADR 0072 — One pinned Rust library for iOS capture

Status: accepted (IOS)

## Context

The capture app needs the same wire, image, evidence and proving logic as the
verifier tooling. Linking independently built `galaxy-prover` and `galaxy-ffi`
static libraries would duplicate Rust `std` and present two Swift FFI and error
surfaces. Swift must persist captures safely across interruptions and send
only protocol transactions, not evidence or proof inputs, to XRPL.

## Decision

- Link one physical-iOS archive, `libgalaxy_ffi.a`. Reuse `galaxy-prover`'s Rust
  `Prover::open_pinned`, proof, verification, timing and progress APIs under the
  single UniFFI namespace `galaxy_ffi`. The separately generated prover bench
  bindings do not ship with the capture app.
- Embed the selected policy JSON and network TOML by path at build time. The
  app supplies its canonical descriptor resource. Require the descriptor ID,
  canonical policy ID and VK digest to match one accepted network entry; pin
  graph and zkey bytes, confirm the loaded VK digest and show the ceremony.
  A single-party development circuit can be explicitly accepted on Testnet,
  never on mainnet (ADR 0043).
- Private state is an immutable Rust evidence object or opaque witness handle.
  Raw and GXEV bytes cross only the local FFI for capture and protected storage.
  Swift owns App Attest, Keychain, journaling and URLSession; the Rust XRPL
  surface builds and parses requests without networking. Typed redacted errors
  cross the FFI instead of downstream error text.
- Swift journals signed transactions before submission, resumes by the same
  blob/hash, and uses one challenge tied to the validated CAPTURE ledger. Rust
  checks the expected public statement and the local proof before producing
  the PROOF memo. Swift waits for that PROOF's validated success before export.

## Consequences

A policy, accepted-network config or descriptor change requires a matched
build and artifact rollout. Mapped proving files must be immutable and kept
available for any outstanding capture pinned to them. The single library and
bindings can be generated on Linux; Swift integration, final signing/linking,
App Attest and real proof on-device still require the Mac and phone. The build
and device integration steps are specified in [ios-ffi.md](../protocol/ios-ffi.md).
