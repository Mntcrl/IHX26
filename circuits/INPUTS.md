# Witness input schema — `GalaxyPhysicalConsistencyProofV1` (galaxy-pc-v1m)

Produced by `galaxy_witness::build_input` (crate `crates/galaxy-witness`), consumed by
`circuits/physical-consistency/main_v1m.circom` (WASM witness calculator and the
`main_v1m.graph.bin` witness graph). Spec references: §6 commitments, §7 sampling,
§9 interface, §20 split and input ranges.

The input JSON contains private evidence (pixels, depth, sensor values, salts). It
stays on the device that proves; it is never logged, uploaded or committed.

## Encoding

- One JSON object, exactly the keys below, no others.
- Every leaf value is a JSON **string** holding a canonical decimal integer in
  `[0, r)`, r = BN254 scalar modulus. Signed values v < 0 are encoded as `r − |v|`.
- Arrays are nested exactly as declared (row-major, outermost index first).
- Indices: slot `s ∈ 0..16` (sampled tile order), quadrant `q ∈ 0..4`, block `b = 4s + q`,
  temporal candidate `c = 2s + qi` (qi 0 → q 0, qi 1 → q 3), temporal pair `p = 2c + ji`
  (ji 0 → frame 0 = t−, ji 1 → frame 2 = t+).

## Public inputs (22, this order; §9)

| key | shape | value |
|---|---|---|
| `rgb_root` | scalar | RGB tree root of frame 1 (unsalted; recomputed by verifiers from the photo) |
| `evidence_root` | scalar | §6 evidence root (published in CAPTURE) |
| `capture_id_hi`, `capture_id_lo` | scalar | `hash_limbs(capture_id)`: high/low 128-bit big-endian halves |
| `seed_hi`, `seed_lo` | scalar | `hash_limbs(seed)`, seed from the selection ledger (§7) |
| `sampled_tiles` | [16] | tile labels `8·ty + tx`: 8 central ascending, then 8 peripheral ascending; central means `2 ≤ tx ≤ 5` and `1 ≤ ty ≤ 4` |

The single source of this layout is `galaxy_crypto::public::public_inputs_v1`.
There are no public outputs.
The verifier derives `seed` and the exact tile list from the validated ledger statement.
The circuit takes those as public inputs: it checks the limb bounds and tile structure
but does not derive the ledger seed or run tile selection.

## Private inputs

| key | shape | content | circuit range checks |
|---|---|---|---|
| `roots` | [3] | frame-tree roots: DEPTH (frame 1), FRAME_MINUS (frame 0), FRAME_PLUS (frame 2) | bound via components → `evidence_root` |
| `salts` | [8] | salts of slots 1..8: depth0, frame_minus, frame_plus, camera, imu, bluetooth, noise, meta | each < 2^248 |
| `camera` | [54] | CAMERA elements (§6): per frame f = 0,1,2: fx, fy, cx, cy (Q16), R00..R22 (Q30), Tx, Ty, Tz (µm), ts_us, track | fx..cy < 2^26; R, T in i32 range; ts < 2^48; track < 4 |
| `imu` | [133] | IMU elements (§6): g0x, g0y, g0z, a_peak_mg, 0, then 128 packed samples `dt·2^96 + (ωx+2^31)·2^64 + (ωy+2^31)·2^32 + (ωz+2^31)` (window 0 then 1) | g0 in i32 range; a_peak < 2^32; element 4 = 0; each sample < 2^128 (dt < 2^32, ω fields 32 bits) |
| `bluetooth` | [24] | BLUETOOTH elements (docs/protocol/evidence-v1.md), opaque | hashed only |
| `noise` | [18] | NOISE elements, opaque | hashed only |
| `meta` | [9] | META elements, opaque | hashed only |
| `blk_rgb` | [16][4][2] | RGB leaf limbs (l0, l1) of block b = 4s+q of frame 1 | each limb < 2^192 (24 bytes) |
| `blk_depth` | [16][4][2] | DEPTH leaf limbs (dl0, dl1) of the same blocks | each limb < 2^144 (8 × 18-bit entries: depth 16 bits, conf 2 bits) |
| `blk_rgb_siblings` | [16][4][3][3] | RGB tree siblings, levels 1..3 (block → tile node), ascending child order without the path child | — |
| `blk_depth_siblings` | [16][4][3][3] | DEPTH tree siblings, levels 1..3 | — |
| `tile_rgb_siblings` | [16][3][3] | RGB tree siblings, levels 4..6 (tile node → root), shared by the 4 blocks of the tile | — |
| `tile_depth_siblings` | [16][3][3] | DEPTH tree siblings, levels 4..6 | — |
| `pr_bx`, `pr_by` | [64] | opened block of pair p in the 64×48 block grid: `(uj>>2, vj>>2)` if inb else `(0, 0)` | bx < 64, by < 48 (padding rows are not openable); equality with the projection is constrained by `GalaxyPolicyV1m` |
| `pr_rgb` | [64][2] | FRAME leaf RGB limbs of that block (frame 0 or 2) | < 2^192 each |
| `pr_depth` | [64][2] | FRAME leaf depth limbs | < 2^144 each |
| `pr_siblings` | [64][6][3] | FRAME tree siblings, levels 1..6 | — |

Leaf packing (§6): RGB block bytes are the 16 pixels in (y, x) row-major order, each
R, G, B; `l0` = big-endian integer of bytes 0..24, `l1` of bytes 24..48. Depth entries
`e_k = depth_k·4 + conf_k`; `dl0 = Σ_{k<8} e_k·2^(18·(7−k))`, `dl1` likewise for k = 8..15.

## Values the circuit derives (not inputs)

ctx, per-slot hashes and their strict 254-bit decompositions, block picks
(`blk_u0`, `blk_v0`), temporal picks (`tmp_ix`, `tmp_iy`), hypothesis indices
(`hyp_idx`), Merkle positions of all openings, and every unpacked byte/depth/
confidence and camera/IMU field passed to `GalaxyPolicyV1m`. The temporal block
coordinates are inputs, not computed by the witness calculator; the policy constrains
them to the projection (or `(0,0)` if out of bounds), and the opening uses those same
coordinates.

## Producer checks (`galaxy_witness::build_input`, fail closed)

`Evidence::validate` (§20 input ranges; never clamps), challenge consistency (ctx,
tiles, picks and hypothesis indices recomputed from the supplied seed and capture id),
view layout and frame ordering, a real-grid temporal block for every pair (forced
to `(0,0)` for an out-of-bounds pair), salts < 2^248, recomputed component commitments
(including the seven fixed padding slots) and evidence root, and every opening
re-hashed to its committed root.
`build_input_from_evidence` builds the reference geometry view before these checks.
