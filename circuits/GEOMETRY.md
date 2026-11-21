# GalaxyPolicyV1m geometry

`components/geometry/policy_v1m.circom` implements protocol §§8.2–8.5 and the §20 interface. The main circuit authenticates the caller-supplied data, proves the §20 ranges and sampling coordinates, and enforces PASS. The standalone policy computes every statistic and predicate even for a failing observation; it does not enforce PASS or authenticate pixels. A GeometryView's `pairs` metadata is not a circuit input.

## Reproduce the checks

From the repository root, with Circom at `.tools/bin/circom` (or `CIRCOM` set), `circuits/node_modules` installed and Node.js 22.18 or later:

```sh
node scripts/circuit-gen-policy.mjs --check
scripts/with-slot.sh --heavy node --test circuits/tests/geometry/policy.test.mjs
```

The suite checks all 19 statistics, five predicate bits and their PASS conjunction against every default GeometryView in `test-vectors/policy/`. The coefficient-overflow view runs in a separate build with the generated `policy-test-dmin1.json` constants; no test-only constants enter the production include directory. For every reference view the suite also checks its R1CS witness. Negative cases modify checked witnesses and run the R1CS again, independently of the witness calculator.

To also test the private calibration data, build `geometry-tool` from the workspace and set `GALAXY_GEOMETRY_TOOL` to its executable path:

```sh
CARGO_TARGET_DIR=target/geometry-test scripts/with-slot.sh cargo build --release -p galaxy-geometry --bin geometry-tool
GALAXY_GEOMETRY_TOOL="$PWD/target/geometry-test/release/geometry-tool" \
  scripts/with-slot.sh --heavy node --test circuits/tests/geometry/policy.test.mjs
```

This runs `geometry-tool view` on the first lexicographically sorted GXRW of each calibration class, with synthetic capture IDs, ledger indices and ledger hashes. It regenerates the GeometryViews under the ignored `fixtures/real/calibration/circuit-geometry-views-*` directories using the current policy. Keep those files private and never commit them. The subtest names and diagnostics omit the capture names and report no private values.

On a policy change, regenerate the Circom constants, protocol policy vector and Rust policy fixtures before rerunning the comparisons; rebuild the main artifacts separately. Do not regenerate while the suite is running: the compiled circuit keeps its thresholds while the private reference tool reads the current policy.

## Constraint measurements

Circom 2.2.3, standalone `GalaxyPolicyV1m`, policy `4ad25185f11bfb52af5dee5a1ab39d4e2428c1c83188c7483db1777d22367073`:

| Optimization | R1CS rows | Purpose |
|---|---:|---|
| `--O1` | 453,217 | Differential and negative tests; more named wires retained |
| `--O2` | 390,865 | Production optimization; 59,135 below the 450,000 policy budget |

The test prints this `--O1` breakdown. It assigns each row to a single group by surviving `.sym` wire names; shared aliases are attributed once, and rows with no unique group go in wiring. This is accounting for the composed harness, not isolated template costs (elimination and shared wires can move rows between groups).

| Sub-gadget / group | R1CS rows |
|---|---:|
| Top-level wiring / mixed | 16,664 |
| Source pixel validity | 41,472 |
| Planarity: top-level, including shared residual-bit wires | 68,103 |
| Planarity: vertex selection and plane fitting | 13,438 |
| Planarity: residual support | 37,892 |
| RGB–LiDAR correspondence | 87,595 |
| Relative pose | 1,728 |
| Temporal: source selection and aggregation | 7,550 |
| Temporal: reprojection, depth and photo pairs | 165,189 |
| Sensor quality | 4,007 |
| Motion checks outside IMU windows | 2,029 |
| IMU windows | 7,550 |
| **Total** | **453,217** |

To measure the production-optimized policy without the main circuit's hashing and range checks, sum the compiler's linear and nonlinear counts:

```sh
mkdir -p target/geometry-measure
scripts/with-slot.sh --heavy .tools/bin/circom circuits/tests/geometry/policy.circom \
  --r1cs --O2 -o target/geometry-measure
```

## Integer ranges and field soundness

Assume only the entry ranges in protocol §20, including the caller-derived block origins, raster coordinates and hypothesis indices. In particular an invalid depth can be 65,535, and invalid observations do not inherit the policy's acceptance thresholds for intrinsics, poses or IMU. `d` is u16, RGB is u8, confidence is 2 bits, intrinsics are u26, rotation/translation/gyro/gravity are i32, timestamps are u48, and IMU `dt` and acceleration are u32. For generated constants use uint64 bounds, a signed-permutation `m_cd`, the exact structural constants, `1 ≤ d_min ≤ d_max < 2^13`, and `0 ≤ occ_pm ≤ 1000`.

### Primitives

- `GeoSignedBits(n)` constrains `x + 2^n` to an `(n+1)`-bit unsigned value, so it encodes `−2^n ≤ x < 2^n` without a field alias. For `GeoLT(n)` the bounded value is **`a − b`**, not either operand by itself. `GeoBitsFor(M)` gives a width where `M < 2^n`. For comparisons of nonnegative operands bounded by `A,B`, `max(A,B)` bounds their difference in magnitude; for signed expressions and a threshold, use the sum of their magnitude bounds. `GeoAbsLe` is closed; `GeoAbsGe` includes both endpoints, including the coincident threshold at zero.
- In `GeoBand(n,k)`, the unsigned `edge` is between 0 and `2^(n−k)`. The product is zero only at the two endpoints, which encode the exact half-open band `[-2^k, 2^k)`. When `k ≥ n`, the entire previously decomposed domain is inside. The only policy band decomposition has `n = 58`, so the product magnitude is at most `2^116`.
- `GeoFloor(qBits,dBits)` and `GeoUnsignedFloor` constrain `num = q·den + r` while decomposing `r` and `den − 1 − r` to `dBits` each. Callers supply a derived `1 ≤ den < 2^dBits`; these decompositions enforce `0 ≤ r < den`. Together with the quotient range there is exactly one integer solution. The plane division uses `(83,57)` and projection uses `(8,134)`; even for arbitrary range-checked quotient wires the identity terms have magnitudes at most `2^142`. `GeoShiftFloor` encodes the remainder in the low bits and computes the signed high portion; it rounds down for negative inputs too.
- IsZero/IsEqual and decompositions derive bits; callers pass bits to Boolean products and muxes. Counts are sums of those bits. The first-maximum mux updates only on a strict increase; the integer ranges of support counts also bound the differences fed to that mux's comparisons. IsZero's auxiliary inverse is a field value, not a signed integer; the magnitude analysis concerns the integer semantics, not field inverses or residue representations.

### Planarity and depth validity (§8.2)

| Expression | Bound and relevant check |
|---|---|
| Pixel validity | `d` and both depth thresholds are in `[0,2^16)`, so each signed difference fits `n=16`; confidence and its threshold use `GeoBitsFor(max(3, conf_min))`. |
| Vertex mux | Each `u < 256`, `v < 192`, `d < 2^16`, and validity bit packs in 33 bits. Global and central indices use 8 and 7 selector bits respectively. |
| Area and plane denominator | Each cross term has magnitude at most `255·191`; `|Δ| ≤ 3·255·191 < 2^18`. The area comparison covers this bound and `area_min`. Raw `Dp < 2^48` and raw `Dp·|Δ| < 2^66`. Only active fits divide by that denominator; all three vertices then have `1 ≤ d < 2^13` and `|Δ| ≥ area_min ≥ 1`, so the constrained denominator is positive and below `2^57`. Inactive fits divide `0` by `1`. |
| Plane numerators and quotients | Each `w_i < 2^32`. The largest determinant bound is `3·2^32·255·191 < 2^50`; multiplying by `P = 2^32` keeps the signed numerator below `2^82` in magnitude. With `den ≥ 1`, floor quotient magnitude is at most the numerator magnitude. The signed width 83 and coefficient threshold comparison width 84 cover these values, also for the test policy. |
| Residual | Degenerate coefficients are zero; all other coefficients have magnitude `< 2^36`. Invalid depths are masked to zero only for residual arithmetic. Thus `|res| < 2^32 + 2^13·2^36·(255+191+1) < 2^58`. Inlier and strong tests share the 59-bit offset decomposition but have their own half-open bands. They also require validity and non-degeneracy. |
| Support and tiles | Support maxima are 256 globally and 128 centrally (signed widths 9 and 8 for differences). Each slot count is at most 16; the total and central counts are bounded by 256 and 128, and the total and central tile counts by 16 and 8. A percentage difference fits the width for `cap·max(100, threshold)`; a count-versus-threshold difference uses `max(cap, threshold)`. |

### RGB–LiDAR and temporal reprojection (§§8.3–8.4)

| Expression | Bound and relevant check |
|---|---|
| Luminance and gradient | `0 ≤ Y ≤ 256·255 = 65,280`. All absolute luminance comparisons include `65,280 + threshold` in their width, including temporal informativeness and photo tolerances. RGB–LiDAR depth differences use `200·65,535 + disc_pct·131,070` including invalid pixel inputs. RGB–LiDAR counts are bounded by 512, with thresholds and percentages handled as in planarity. |
| Pose products and floor | An `R·R` product has magnitude at most `2^62`; `|T_1−T_j| < 2^32`, so an `R·(T_1−T_j)` product has magnitude `< 2^63`. The three-term sums fit signed widths 64 and 65 respectively. Shifting by 30 gives `|R_j1| < 2^34`, `|T_j1| < 2^35`, and `|vee| < 2^35`. |
| Source and projection | `|A_0|, |B_0| < 2^26`, `F < 2^52`, `|P_a| < 2^68`, `2^30·F < 2^82`; hence `|Q_a| < 3000·2^102 + 2^117 < 2^118`. The sign comparison uses 118 and `|den| = 2^16·|Q_z| < 2^134`. Perspective products are `< 2^144` in magnitude, and `(c + 2^15)·Q_z` is `< 2^145`, so `|num| < 2^146`. The difference against `bound·den` fits 147 since `bound ≤ 256`. |
| Raster guard | When `Q_z > 0`, `0 ≤ num < bound·den` is exactly the half-open raster test for the floored coordinate. When that test fails or `Q_z ≤ 0`, divide `0` by `1`. The guarded denominator is positive and `< 2^134`, and each quotient fits 8 bits. The high bits enforce `pr_bx, pr_by`; the low bits select the corresponding pixel. |
| Occlusion and depth consistency | `M = 1000·2^30·F < 2^92`; with raw target depth `dj`, `dm = dj·M < 2^108`. The difference between `1000·dm` and `(1000−occ_pm)·Q_z` fits signed width 129, and `|1000·(dm−Q_z)| < 2^129`. The inclusive depth comparison includes the bound `dcons_pm·2^108` in its width. All aggregates are at most 64 and percentage checks use that cap, including the nonnegative `N_usable−N_occ`. |

### Quality and motion (§8.5)

| Expression | Bound and relevant check |
|---|---|
| Tracking, intrinsics, frame intervals | Tracking uses equality. Intrinsic comparisons use `max(2^26, lo, hi)`. A frame timestamp difference, even in reverse order, has magnitude `< 2^48`; the closed interval width includes `2^48 + max(lo, hi)`. |
| Orthonormality and handedness | A three-term row inner product minus its identity constant has magnitude `< 2^64`; include `ortho_tol_q60` in the absolute comparison width. A row cross term is at most `2^63` in magnitude; its product with a third row entry is at most `2^94`. The determinant magnitude is at most `3·2^94 < 2^96`, covering the strict positive test. |
| Baseline and rotation | Each relative translation has magnitude `< 2^35`, so its squared sum is `< 3·2^70 < 2^72`. Compare against the nonnegative squared baseline. Relative rotation's `vee` uses `2^35 + rot_max_q30` for the absolute comparison. |
| IMU validity and coverage | Sample duration comparison uses `max(2^32, imu_dt_max_us)`; padding and rate checks use IsZero and Boolean gates. At most 64 samples contribute, total duration `< 2^38`, and subtracting the signed frame interval gives magnitude `< 2^49`. Include `imu_cover_tol_us` in the coverage comparison width. |
| IMU integration and gyro | `|ω·dt| < 2^63`, `|Θ| < 2^69`. The signed permutation preserves component magnitude. `|10^6·vee ± 2^11·Θc| < 2^55 + 2^80 < 2^81`. Include `2·gyro_tol_urad·2^30` in the comparison width. |
| Gravity and acceleration | `|gc|, |gp| ≤ 2^31`; the norm minus `2^40` has magnitude `< 2^64`. The scaled gravity dot product has magnitude `≤ 3000·2^62 < 2^74`; compare using `2^74 + grav_cos_milli·2^50`. Acceleration and the threshold are nonnegative, with difference bounded by `max(2^32, acc_max_mg)`. |

The widest policy-dependent comparison bound above is `2^129 + dcons_pm·2^108 < 2^173`; an offset decomposition needs at most 174 bits under the generator's integer bounds. All integer expressions and the bounded terms in their constraints are far below `2^250` in magnitude; field arithmetic therefore agrees with the integer relations used to derive the outputs.
