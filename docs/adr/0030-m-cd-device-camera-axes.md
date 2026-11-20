# ADR 0030 — Calibration of m_cd (device → CV camera axes)

Status: accepted (2026-11-20, approved by the orchestrator)

## Context
- §8.5 maps CoreMotion device-frame vectors (gyro integral Θ_w, gravity g0) into CV camera axes with the signed permutation `motion.m_cd` from the policy; the spec requires the value to be validated on device.
- The initial value `[0,1,0, 1,0,0, 0,0,−1]` assumed CV x = device y and CV y = device x.
- Two real GXRW captures from an iPhone 13 Pro Max (`fixtures/real/capture-ios/`, private) were fitted with `geometry-tool imu-frame`, which compares vee(R_j1) from ARKit poses with the integrated gyro for all 48 signed permutations, and the gravity predicate was evaluated independently. Measurements: `docs/experiments/real-captures-v1m.md`.

## Decision
- Set `motion.m_cd = [0,−1,0, −1,0,0, 0,0,−1]` in `policies/galaxy-pc-v1m.json` (CV x = −device y, CV y = −device x, CV z = −device z). No other policy value changes.
- The policy_id changes from `4f6851de…a882` to `ef744a7f28dc606b117dbab34899753a218cd1865b4fc69a9739ac9c10a7afac`; the generated circom policy constants, `test-vectors/policy/` and every policy_id-pinned vector are regenerated.

## Consequences
- With the new value the gravity alignment of both real captures is cos 0.991 / 0.993 (threshold 0.950); with the old value it was −0.922 / −0.744, so every real capture failed motion_ok.
- Both captures agree on the same best permutation (combined RMS 33 mrad vs 97 mrad for the runner-up). The residual still exceeds gyro_tol_urad (20 mrad) because these captures exceed the motion envelope (relative rotation up to 26°, baselines 91–228 mm); gyro_tol_urad, rot_max_q30 and the baseline envelope stay unchanged until a small-motion capture is available for calibration.
- Synthetic captures derive their IMU from the policy value, so synthetic results are unaffected by the change of convention.
