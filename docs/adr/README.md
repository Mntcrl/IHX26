# Architecture Decision Records

Format: Context / Decision / Consequences. Ranges per agent are listed in spec §1 and §20.

| ADR | Title |
|---|---|
| [0001](0001-monorepo-and-history.md) | Monorepo and imported iOS history |
| [0002](0002-shared-rust-core.md) | Shared Rust core and pinned dependency set |
| [0003](0003-mobile-profile-v1m.md) | Mobile profile `galaxy-pc-v1m` |
| [0004](0004-commitment-trees.md) | Commitment trees |
| [0005](0005-image-profiles.md) | Image profiles |
| [0006](0006-trust-model.md) | Trust model |
| [0007](0007-app-attest-serverless.md) | Serverless App Attest |
| [0008](0008-build-and-device-workflow.md) | Build and device workflow |
| [0009](0009-network-policy.md) | Network policy |
| [0010](0010-poseidon-arkworks-pinning.md) | Poseidon and arkworks pinning |
| [0011](0011-field-and-limb-encoding.md) | Field and limb encoding |
| [0012](0012-sampling-algorithm.md) | Sampling algorithm details |
| [0013](0013-wire-strictness.md) | Wire strictness rules |
| [0020](0020-jpeg-reference-decoder.md) | jpeg-v1 reference decoder and cross-target determinism |
| [0021](0021-arkit-ingest-quantization.md) | ARKit ingest quantization |
| [0022](0022-bluetooth-noise-meta-layouts.md) | BLUETOOTH / NOISE / META component layouts |
| [0023](0023-analysis-raster-alignment.md) | Analysis raster alignment |
| [0030](0030-m-cd-device-camera-axes.md) | Calibration of m_cd (device → CV camera axes) |
| [0040](0040-circuit-core-composition.md) | Circuit core composition (galaxy-pc-v1m) |
| [0041](0041-witness-input-builder.md) | Witness input schema and `galaxy-witness` |
| [0042](0042-constraint-level-negative-tests.md) | Constraint-level negative tests |
| [0043](0043-groth16-setup-ceremony.md) | Groth16 setup: development key now, multi-party ceremony for production |
| [0044](0044-circuit-build-artifacts.md) | Circuit build pipeline, artifact digests and the circuit bundle |
| [0050](0050-xrpl-client-and-submission.md) | XRPL codec, ledger access and reliable submission |
| [0060](0060-apple-app-attest-verification.md) | Verify the Apple App Attest wire profile |
| [0070](0070-app-attest-development-environment.md) | App Attest in the development environment without the entitlement |
| [0071](0071-aqua-session-build-runner.md) | Aqua-session build runner for signed iOS builds |
| [0080](0080-mobile-prover-backend.md) | Mobile proving backend |
| [0090](0090-verifier-states-and-proof-selection.md) | Verifier states, development credentials and PROOF selection |
| [0091](0091-offline-recordings-and-cli-discovery.md) | Offline XRPL recordings, verifier test vectors and CLI discovery |

Ranges: orchestrator 0001-0009, ProtocolCrypto 0010-0019, ImageEvidence 0020-0029, Geometry 0030-0039, CircuitCore 0040-0044, CircuitGeo 0045-0049, XRPL 0050-0059, Attestation 0060-0069, IOS 0070-0079, MobileProver 0080-0089, Verifier 0090-0099, Node 0100-0109, WebVerifier 0110-0119, Security 0120-0129.
