# ADR 0012 — Sampling algorithm details

Status: accepted (2026-11-20). Normative text: spec §3, §7. Owner: ProtocolCrypto.

## Context
- The challenge must be unpredictable at capture time (future ledger L0 + 5), identical for every verifier, unbiased, and cheap to bind inside the circuit.
- Tile selection is outside the circuit (public inputs); block and point selection is inside (Poseidon bits).

## Decision
- Seed = SHA256("GALAXY/SAMPLE/v1\0" ‖ u32be network ‖ capture_id ‖ u32be Lsel ‖ ledger_hash(Lsel)); the word stream is SHA256("GALAXY/SAMPLE-TILES/v1\0" ‖ seed ‖ u32be counter) for counter 0, 1, …, each block split into eight u32be words. One stream serves both strata in order (central first).
- Stratified partial Fisher–Yates: 8 picks from the 16 central tiles, then 8 from the 32 peripheral tiles, rejection sampling with limit = ⌊2^32/n⌋·n (unbiased modulo). Output = sorted central picks ‖ sorted peripheral picks, so the circuit can check strict ascending order and stratum membership instead of re-running the shuffle.
- In-circuit picks: ctx = Poseidon5(TAG(10,0), capture limbs, seed limbs); slot hash h_s = Poseidon3(TAG(11,s), ctx, T_s); hypotheses g_j = Poseidon2(TAG(12,j), ctx). All bit fields come from the 254 canonical little-endian bits (ADR 0011). Per quadrant: 4 bits for the block, 2 bits for the temporal pick; hypotheses take 8-bit (global) or 7-bit (central) indices.
- Tests drive the shuffle with injected word streams (rejection at x ≥ limit, n = 1, exhaustion order) and with ≥ 20 seeded cases checked against an independent Node.js implementation (`sampling.json`).

## Consequences
- Each verifier reproduces the public tiles from ledger data alone; a prover cannot choose tiles because Lsel is in the future at capture time and retries are not allowed.
- Index collisions in hypotheses are allowed and handled by the policy (degenerate hypothesis), not resampled.
