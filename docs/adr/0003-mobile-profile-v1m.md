# ADR 0003 — Mobile profile `galaxy-pc-v1m`

Status: accepted (2026-11-20). Normative text: spec §8, §18.1–§18.2.

## Context
- arch.docx targets 48 tiles × 64 blocks × 3 frames (9,216 sampled blocks) and 256 correspondences per proof. The proof must be produced on an iPhone 13 Pro Max (6 GB RAM) with Groth16 over BN254; the Mac build host has 8 GB RAM (recon §8).
- Groth16 proving memory and time grow roughly linearly with constraints; a phase-1 setup of 2^20 (and 2^21 headroom) points is the planned budget (ptau files in `circuits/ptau.lock.json`). [INFERENCE] The full target is far above 2^21 constraints once Poseidon openings for 9,216 blocks are counted.
- Planarity in the central region of interest is only well defined if central tiles are guaranteed to be sampled.

## Decision
- The mobile profile opens 16 tiles selected from the future ledger seed: 8 from the 16-tile central stratum and 8 from the 32 peripheral tiles (partial Fisher–Yates, spec §7), 4 blocks per tile (one per quadrant) of frame t0 with all four interior pixels evaluated, plus 64 data-dependent temporal openings in frames t− and t+.
- Plane hypotheses: 4 global (over 256 points) + 4 central (over 128 points), indices from Poseidon-derived bits.
- Inlier thresholds are powers of two (`eps_log2`, `sep_log2`) so the circuit range checks residuals by bit decomposition.
- The full arch.docx target is estimated from measured per-component constraint counts and documented, not built as the mobile profile (spec §8.9).

## Consequences
- Soundness depends on the sampling rate: a forger who fakes a fraction of the scene escapes detection with a probability determined by 16/48 tiles and 4/64 blocks per tile; Security quantifies this in `docs/security/`.
- Policy values may change with calibration (new `policy_id`/`circuit_id`); formulas change only through a spec update.
- Constraint counts and proving memory must be measured (PROGRESS criterion 20) before the profile is considered final.
