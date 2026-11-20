# ADR 0011 — Field and limb encoding

Status: accepted (2026-11-20). Normative text: spec §2, §6, §9. Owner: ProtocolCrypto.

## Context
- BN254 scalars travel on the XRPL ledger (CAPTURE `rgb_root`, `evidence_root`), in JSON vectors and witness files, and as public inputs. A decoder that reduces modulo r accepts two byte strings for one value (malleability) and breaks byte-exact ids such as `capture_id`.
- SHA-256 outputs (capture_id, seed) are 256-bit and do not fit one field element.

## Decision
- Wire scalars: 32 bytes big-endian, value < r; `fr_from_be_canonical` and the CAPTURE decoder reject ≥ r. Decimal strings (`fr_from_dec`): ASCII digits only, no sign, no leading zero, value < r.
- Signed integers enter the field only through `fr_from_i128` (x mod r); this is the only reduction in the codebase.
- 256-bit hashes → two 128-bit limbs: `hi` = BE integer of bytes 0..16, `lo` = bytes 16..32 (`hash_limbs`). Limbs are always < 2^128 < r, so no information is lost and the circuit can range-check them.
- Bits of a field element in-circuit and in Rust: the 254 canonical little-endian bits of the reduced value (`canonical_le_bits`); the circuit enforces the strict decomposition (alias-free).
- Base-field coordinates of Groth16 points: 32 bytes BE, value < q, checked separately from r.

## Consequences
- Every scalar has exactly one encoding in each representation; vectors in `field.json` cover 0, 1, 2^128, r−2^127, r−1, r and 2^256−1.
- Producers that compute values outside the field (e.g. raw SHA-256 outputs) must use limbs; they cannot shortcut through a modular reduction.
