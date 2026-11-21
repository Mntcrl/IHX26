# Constraint report — `GalaxyPhysicalConsistencyProofV1` (galaxy-pc-v1m)

Compiler: circom 2.2.3, `--O2 --prime bn128`. Policy: `policies/galaxy-pc-v1m.json`,
policy_id `4ad25185f11bfb52af5dee5a1ab39d4e2428c1c83188c7483db1777d22367073`
(generated include `physical-consistency/generated/policy_v1m.circom`).
Source tree digest (`build.json`): `a9818f2efe11275b12f77b7ac5f8ec810e33a77f8c1021d2434dedb6f31ff5c8`.

Refresh with `scripts/circuit.sh build` (full circuit), then
`scripts/circuit.sh constraints` (each production template compiled alone as
`main` with the same flags). The final policy changed the compiled R1CS digest
without changing the full or per-template constraint counts.

## Full circuit (`artifacts/v1m/r1cs-info.json`)

| item | value |
|---|---|
| constraints (all non-linear after `--O2`) | **876,639** (< 2^20 = 1,048,576; headroom 171,937) |
| wires | 868,094 |
| public inputs / public outputs | 22 / 0 |
| private inputs | 3,481 |
| labels | 2,841,649 |
| template instances | 434 |
| Groth16 domain | 2^20 (876,639 + 22 + 1 ≤ 2^20; pinned ptau power 20) |

## Per component

Counts are per instance, compiled standalone (inputs free). "Total" = per
instance × instances in `main_v1m`.

| template | role (spec) | per instance | instances | total |
|---|---|---:|---:|---:|
| `GalaxySampling()` | §7: tile checks on the public labels, ctx Poseidon5, 16 slot hashes Poseidon3, strict 254-bit decompositions, block/temporal picks, hypothesis indices | 19,730 | 1 | 19,730 |
| `GalaxyCenterTile(1)` | §6/§20: RGB tile — 4 block leaves (48 bytes, 8-bit checks), block-in-tile openings and the shared tile-to-root path | 7,497 | 16 | 119,952 |
| `GalaxyCenterTile(2)` | same for the DEPTH tree (16-bit depth, 2-bit confidence from 18-bit entries) | 7,113 | 16 | 113,808 |
| `GalaxyTemporalBlock(0)` | FRAME_MINUS opening (depth 6, data-dependent Morton position from pr_bx/pr_by) + leaf unpacking | 2,971 | 32 | 95,072 |
| `GalaxyTemporalBlock(2)` | FRAME_PLUS, same | 2,971 | 32 | 95,072 |
| `GalaxyEvidenceCommitment()` | §6: 8 component commitments (DEPTH0/FRAME±/CAMERA/IMU/BLUETOOTH/NOISE/META), padding slots, evidence_root | 25,165 | 1 | 25,165 |
| `GalaxyCamera()` | 54 CAMERA elements: signed ranges (offset + Num2Bits) for fx/fy/cx/cy/R/T/ts/track | 1,614 | 1 | 1,614 |
| `GalaxyImu()` | 133 IMU elements: 128 packed samples unpacked to dt/ω with range checks | 16,512 | 1 | 16,512 |
| `GalaxyPolicyV1m()` | CircuitGeo predicates (breakdown below) | 390,865 | 1 | 390,865 |
| `main` body | `PASS` = product of the five policy flags (4 products), `PASS === 1` | 4 | 1 | 4 |
| **sum of standalone counts** | | | | **877,794** |
| **full circuit** | | | | **876,639** |

The full circuit has 1,155 constraints fewer than the standalone sum: when the
templates are composed, `--O2` turns some constraints whose inputs become
linearly tied across template boundaries (equalities with public inputs and
roots, values shared between components) into linear ones and eliminates them.
[INFERENCE] The exact per-template attribution of these 1,155 eliminations was not
measured.

### `GalaxyPolicyV1m()` breakdown (owner CircuitGeo, `components/geometry/`)

| template | per instance | instances | total |
|---|---:|---:|---:|
| `GeoQuality()` | 3,677 | 1 | 3,677 |
| `GeoRelativePose()` | 1,644 | 1 | 1,644 |
| `GeoMotion()` | 8,734 | 1 | 8,734 |
| `GeoPlanarity()` | 124,308 | 1 | 124,308 |
| `GeoRgbDepth()` | 66,589 | 1 | 66,589 |
| `GeoTemporal()` | 155,967 | 1 | 155,967 |
| `GeoPixelValid()` (policy body: 64 blocks × 12 non-corner pixels) | 39 | 768 | 29,952 |
| sum | | | 390,871 |
| `GalaxyPolicyV1m()` standalone | | | 390,865 |

## Shares

| group | constraints | share of full |
|---|---:|---:|
| Merkle openings + leaf unpacking (RGB/DEPTH tiles, FRAME±) | 423,904 | 48.4 % |
| geometry policy | 390,865 | 44.6 % |
| commitments + camera + IMU scalars | 43,291 | 4.9 % |
| sampling | 19,730 | 2.3 % |

## `circom --inspect` warnings (`artifacts/v1m/inspect.log`)

82 warnings, none in a CircuitCore template:

| warning | template | count | assessment |
|---|---|---:|---|
| CA02 | `CompConstant(r−1)` (circomlib, inside `Num2Bits_strict`/`AliasCheck`) | 1 | circomlib: the comparison uses the bits through linear combinations only; standard for `AliasCheck` |
| CA02 | `LessThan(6)` (circomlib) | 1 | circomlib: only the top bit of the `Num2Bits` output is used; the other bits are still constrained by `Num2Bits` |
| CA01 | `GalaxyPolicyV1m()`: `blk_c` (256 signals) | 1 | the confidences of the 4 corner pixels of each of the 64 sampled blocks are not used by the policy (only the 12 non-corner pixels are). They are still bound: main feeds them from the DEPTH leaves, whose unpacking (2-bit check) and Merkle opening constrain them |
| CA01/CA02 | geometry gadgets (`GeoLT`, `GeoFloor`, `GeoPlanarity`, `GeoPlane`, `GeoTemporal`, …) | 79 | CircuitGeo scope (`components/geometry/`): unused bits of range-check decompositions and unused subcomponent outputs |

The core components (`components/core/`) and `main_v1m.circom` contain no
witness-only assignment (`<--`): every signal is set with `<==`.
