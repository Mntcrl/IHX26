# ADR 0040 — Circuit core composition (galaxy-pc-v1m)

Status: accepted (CircuitCore)

## Context
`GalaxyPhysicalConsistencyProofV1` (spec §9, §20) must bind the policy statistics
computed by `GalaxyPolicyV1m` (CircuitGeo) to the committed evidence and to the
ledger challenge, within the on-device budget of about 2^20 constraints. Every value
that influences PASS must be either a public input or derived from constrained
values (§17).

## Decision
- **Sampling is derived in-circuit.** ctx = Poseidon5(TAG(10,0), capture limbs,
  seed limbs) with every limb range-checked < 2^128; per slot h_s = Poseidon3(TAG(11,s),
  ctx, T_s) and per hypothesis g_j = Poseidon2(TAG(12,j), ctx) are decomposed with
  `Num2Bits_strict` (canonical 254 bits, AliasCheck). Block picks, temporal picks and
  hypothesis indices passed to the policy are linear combinations of these bits; the
  prover supplies none of them.
- **Tile selection checks on public inputs.** Each label is decomposed into 6 bits
  (x = bits 0..2, y = bits 3..5); y < 6 (so label < 48), stratum membership via
  central_x·central_y === (slot < 8), strict ascending order within each stratum
  (`LessThan(6)` on adjacent labels).
- **Frame-1 openings share the tile path.** For each sampled tile and each of the RGB and
  DEPTH trees, the four sampled blocks are hashed to the level-3 tile node
  independently and must agree; one tile-to-root path (levels 4..6) is then checked.
  Merkle positions are the derived Morton bits; node tags carry kind, frame, level and
  index exactly as §6. Padding tiles are excluded because tile labels are < 48.
- **Temporal openings** use the policy's prover-supplied block coordinates
  (pr_bx, pr_by), range-checked to the 64×48 grid (bx < 64 via 6 bits, by < 48 by
  excluding bits 4 and 5 both set) and converted to the Morton leaf slot; the policy
  constrains them to the computed projection or (0,0) when not in bounds. Leaf tags
  include the frame so a frame-0 block cannot be opened as frame 2.
- **Leaf unpacking** decomposes RGB limbs (192 bits) and depth limbs (144 bits)
  into bits once; bytes, depth (16 bits) and confidence (2 bits) are linear
  combinations of those bits, which is also the §20 entry range check.
- **Scalars**: CAMERA and IMU elements are hashed into their components and the same
  signals feed the policy after range checks (fx..cy < 2^26, R/T/g0 via
  Num2Bits(32)(x + 2^31), ts < 2^48, track < 4, a_peak < 2^32, IMU samples < 2^128
  with dt and ω fields extracted from the bits; IMU element 4 === 0). Bluetooth, noise
  and meta are hashed only. Salts are range-checked < 2^248.
- **PASS** = quality_ok · geometry_ok · motion_ok · rgb_depth_ok · temporal_ok === 1;
  commitments and selection are hard constraints (any mismatch makes the R1CS
  unsatisfiable). No public outputs.

## Consequences
- Measured total: 905,931 constraints at `--O2` (see `circuits/CONSTRAINTS.md`).
- The core contributes no `<--` assignments of its own; all hints are inside circomlib
  templates (Num2Bits, LessThan, AliasCheck), each constrained by its template.
- Changing tile layout, packing or tag layout requires a new circuit_id.
