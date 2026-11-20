//! Reference `GeometryView` fixtures for the circuit differential tests
//! (`test-vectors/policy/`). Every fixture is built through the same view
//! builder as real evidence, annotated with `expected` by the reference
//! engine, and checked against the predicate or edge case it targets: if the
//! engine no longer reaches a target, generation fails instead of writing a
//! fixture that silently stopped covering it. Sensor data here is synthetic
//! or hand-constructed; none of it is device evidence.

use galaxy_crypto::sampling::Challenge;
use galaxy_evidence::{CameraFrame, FrameData, ImuEvidence, ImuSample};
use galaxy_protocol::policy::Policy;

use crate::lidar::{hypothesis_fits, Fit};
use crate::synthetic::{self, Scenario};
use crate::view::view_from_parts;
use crate::{try_evaluate, GeometryView, PolicyReport, ViewError, HEIGHT, IMU_MAX, WIDTH};

/// Directory (relative to `test-vectors/policy/`) of the isolated test-only
/// coefficient-bound profile; default-policy fixtures are the top-level files.
pub const COEFFICIENT_DIR: &str = "coefficient-overflow";
/// Test-only policy name; never accepted by any configuration.
pub const COEFFICIENT_POLICY_NAME: &str = "galaxy-pc-test-dmin1";
const SEARCH_SEEDS: u64 = 64;
const Q30: i64 = 1 << 30;
const Q20: i32 = 1 << 20;

pub struct Fixture {
    /// File stem; the file is `<name>.json`.
    pub name: String,
    /// Predicate or edge case the fixture targets (README text).
    pub target: String,
    /// View with `expected` filled in.
    pub view: GeometryView,
}

#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    #[error("view construction failed: {0}")]
    View(#[from] ViewError),
    #[error("fixture {0} does not reach its target")]
    Target(&'static str),
    #[error("policy: {0}")]
    Policy(#[from] galaxy_protocol::policy::PolicyError),
}

type Parts = ([FrameData; 3], [CameraFrame; 3], ImuEvidence);

fn annotate(view: GeometryView, policy: &Policy) -> Result<(GeometryView, PolicyReport), FixtureError> {
    let report = try_evaluate(&view, policy)?;
    let mut view = view;
    view.expected = Some(report);
    Ok((view, report))
}

fn ensure(cond: bool, name: &'static str) -> Result<(), FixtureError> {
    if cond { Ok(()) } else { Err(FixtureError::Target(name)) }
}

fn fixture(name: &str, target: impl Into<String>, view: GeometryView) -> Fixture {
    Fixture { name: name.to_owned(), target: target.into(), view }
}

/// All default-policy fixtures (top-level `test-vectors/policy/*.json`).
pub fn reference_fixtures(policy: &Policy) -> Result<Vec<Fixture>, FixtureError> {
    let mut out = Vec::new();
    let room_seed = synthetic_fixtures(policy, &mut out)?;
    room_fixtures(policy, room_seed, &mut out)?;
    plane_fixtures(policy, &mut out)?;
    extreme_fixtures(policy, &mut out)?;
    Ok(out)
}

fn synthetic_view(scenario: Scenario, seed: u64, policy: &Policy) -> Result<GeometryView, FixtureError> {
    let (frames, camera, imu) = synthetic::capture(scenario, seed, &policy.motion.m_cd);
    Ok(view_from_parts(&frames, &camera, &imu, &synthetic::synthetic_challenge(seed))?)
}

/// First seed in `0..SEARCH_SEEDS` whose view reaches `target`.
fn first_seed(scenario: Scenario, policy: &Policy, name: &'static str, target: impl Fn(&GeometryView, &PolicyReport) -> bool)
    -> Result<(u64, GeometryView), FixtureError>
{
    for seed in 0..SEARCH_SEEDS {
        let (view, report) = annotate(synthetic_view(scenario, seed, policy)?, policy)?;
        if target(&view, &report) {
            return Ok((seed, view));
        }
    }
    Err(FixtureError::Target(name))
}

fn synthetic_fixtures(policy: &Policy, out: &mut Vec<Fixture>) -> Result<u64, FixtureError> {
    let (room_seed, view) = first_seed(Scenario::Room, policy, "synthetic-room-pass", |_, r| r.pass)?;
    out.push(fixture("synthetic-room-pass",
        format!("Synthetic room/desk scene, seed {room_seed} (first passing seed): every predicate true, PASS."), view));
    let cases: [(&'static str, Scenario, &str, fn(&GeometryView, &PolicyReport) -> bool); 5] = [
        ("synthetic-flat-wall", Scenario::FlatWall, "Flat textured wall: geometry_ok false (planarity).",
            |_, r| !r.geometry_ok),
        ("synthetic-low-texture", Scenario::LowTexture, "Uniform grey texture over a 3D room: rgb_depth_ok false (no luminance edges at depth discontinuities).",
            |_, r| !r.rgb_depth_ok && r.geometry_ok),
        ("synthetic-dynamic", Scenario::Dynamic, "Object moving between frames: occluded temporal pairs (N_occ > 0).",
            |_, r| r.stats.n_occ > 0),
        ("synthetic-static", Scenario::Static, "Static camera: motion_ok false (baseline below baseline_min_um).",
            |_, r| !r.motion_ok),
        ("synthetic-swapped-frames", Scenario::SwappedFrames, "t- and t+ frame data swapped: temporal_ok false.",
            |_, r| !r.temporal_ok),
    ];
    for (name, scenario, text, target) in cases {
        let (seed, view) = first_seed(scenario, policy, name, target)?;
        out.push(fixture(name, format!("{text} Seed {seed} (first seed reaching the target)."), view));
    }
    Ok(room_seed)
}

fn room_view(parts: &Parts, seed: u64, policy: &Policy) -> Result<(GeometryView, PolicyReport), FixtureError> {
    let (frames, camera, imu) = parts;
    annotate(view_from_parts(frames, camera, imu, &synthetic::synthetic_challenge(seed))?, policy)
}

/// Raster index of point `i` (§8.1) of a view.
fn point_pixel(v: &GeometryView, i: usize) -> usize {
    let (block, k) = (i / 4, i % 4);
    let x = usize::from(v.blk_u0[block]) + 1 + k % 2;
    let y = usize::from(v.blk_v0[block]) + 1 + k / 2;
    WIDTH * y + x
}

fn room_fixtures(policy: &Policy, seed: u64, out: &mut Vec<Fixture>) -> Result<(), FixtureError> {
    let base: Parts = synthetic::capture(Scenario::Room, seed, &policy.motion.m_cd);
    let (pass_view, pass) = room_view(&base, seed, policy)?;

    let mut parts = base.clone();
    parts.1[2].track = 1;
    let (view, r) = room_view(&parts, seed, policy)?;
    ensure(!r.quality_ok && r.geometry_ok && r.rgb_depth_ok && r.temporal_ok && r.motion_ok, "room-tracking-limited")?;
    out.push(fixture("room-tracking-limited", "Passing room with frame t+ tracking state 1 (limited): only quality_ok false.", view));

    let mut parts = base.clone();
    parts.0[1].depth_mm.fill(0);
    parts.0[1].conf.fill(0);
    let (view, r) = room_view(&parts, seed, policy)?;
    ensure(r.stats.v_all == 0 && r.stats.n_hyp_ok_g == 0 && r.stats.n_hyp_ok_c == 0 && r.stats.n_usable == 0
        && r.stats.n_disc == 0 && !r.geometry_ok && !r.rgb_depth_ok && !r.temporal_ok, "room-all-invalid-depth")?;
    out.push(fixture("room-all-invalid-depth",
        "Passing room with every frame-t0 depth invalid (0, confidence 0): V_all = 0, every hypothesis degenerate, N_disc = N_usable = 0; geometry, rgb_depth and temporal false.", view));

    // Degenerate hypotheses: override the challenge-derived indices (the
    // circuit takes hyp_idx as an input, so the differential test does not
    // need them to be challenge-consistent).
    let lidar = &policy.lidar;
    let valid_point = |v: &GeometryView, i: usize| {
        let (b, k) = (i / 4, i % 4);
        let px = 4 * (1 + k / 2) + 1 + k % 2;
        crate::valid(v.blk_d[b][px], v.blk_c[b][px], lidar)
    };
    let invalid_point = 17;
    let mut parts = base.clone();
    let pixel = point_pixel(&pass_view, invalid_point);
    parts.0[1].depth_mm[pixel] = 0;
    parts.0[1].conf[pixel] = 0;
    let (mut view, _) = room_view(&parts, seed, policy)?;
    let small_area = |range: std::ops::Range<usize>| range.map(|b| 4 * b)
        .find(|&i| i / 4 != invalid_point / 4 && (i..i + 3).all(|n| valid_point(&view, n)))
        .ok_or(FixtureError::Target("room-degenerate-hypotheses"));
    let (g, c) = (small_area(32..64)?, small_area(0..32)?);
    let [h3, h7] = [view.hyp_idx[3], view.hyp_idx[7]];
    view.hyp_idx = [
        [3, 3, 200], [invalid_point as u16, 100, 200], [g as u16, g as u16 + 1, g as u16 + 2], h3,
        [5, 90, 5], [invalid_point as u16, 60, 120], [c as u16, c as u16 + 1, c as u16 + 2], h7,
    ];
    let fits = hypothesis_fits(&view, lidar);
    ensure((0..3).chain(4..7).all(|j| fits[j] == Fit::PreDegenerate) && !valid_point(&view, invalid_point)
        && [g, c].iter().all(|&i| (i..i + 3).all(|n| valid_point(&view, n))), "room-degenerate-hypotheses")?;
    let (view, _) = annotate(view, policy)?;
    out.push(fixture("room-degenerate-hypotheses",
        format!("Room with overridden hyp_idx: j=0/4 equal indices, j=1/5 an invalid vertex (point {invalid_point}, depth 0), j=2/6 three valid points of one block (|Δ| = 1 < area_min); j=3/7 challenge-derived."), view));

    let mut view = pass_view.clone();
    view.hyp_idx[..4].copy_from_slice(&[[0, 0, 1], [7, 9, 7], [invalid_point as u16; 3], [11, 12, 11]]);
    let (view, r) = annotate(view, policy)?;
    ensure(r.stats.n_hyp_ok_g == 0 && r.stats.j_star == 0 && r.stats.max_all == 0 && r.stats.n_strong == 0
        && r.stats.n_hyp_ok_c == pass.stats.n_hyp_ok_c && !r.geometry_ok, "room-global-hypotheses-degenerate")?;
    out.push(fixture("room-global-hypotheses-degenerate",
        "Passing room with all four global hypotheses degenerate (equal indices): n_hyp_ok_g = 0, j_star = 0 is degenerate so every valid point is an outlier and none is strong; geometry_ok false.", view));
    Ok(())
}

/// Hand-built rig over a fronto-parallel plane with exact Q30 rotations.
#[derive(Clone)]
struct Rig {
    depth: [u16; 3],
    r: [[i64; 9]; 3],
    t: [[i64; 3]; 3],
    intrinsics: [[i64; 4]; 3],
    ts: [u64; 3],
    windows: [Vec<ImuSample>; 2],
    g0: [i32; 3],
}

const IDENTITY: [i64; 9] = [Q30, 0, 0, 0, Q30, 0, 0, 0, Q30];
/// 180° about the camera y axis (proper rotation).
const TURNED: [i64; 9] = [-Q30, 0, 0, 0, Q30, 0, 0, 0, -Q30];
const INTRINSICS: [i64; 4] = [210 << 16, 210 << 16, 8_355_840, 6_258_688];

fn window(samples: &[(u32, [i32; 3])]) -> Vec<ImuSample> {
    let mut w: Vec<ImuSample> = samples.iter().map(|&(dt_us, w)| ImuSample { dt_us, w }).collect();
    w.resize(IMU_MAX, ImuSample::default());
    w
}

impl Rig {
    fn new() -> Self {
        let still = window(&[(15_000, [0; 3]); 10]);
        Self {
            depth: [1000; 3],
            r: [IDENTITY; 3],
            t: [[-4000, 0, 0], [0; 3], [4000, 0, 0]],
            intrinsics: [INTRINSICS; 3],
            ts: [1_000_000, 1_150_000, 1_300_000],
            windows: [still.clone(), still],
            // m_cd maps device x to camera y; gravity points along camera +y (down).
            g0: [-Q20, 0, 0],
        }
    }

    fn view(&self, seed: u64) -> Result<GeometryView, FixtureError> {
        let frames = self.depth.map(|d| {
            let mut f = FrameData { rgb: Vec::with_capacity(3 * WIDTH * HEIGHT), depth_mm: vec![d; WIDTH * HEIGHT], conf: vec![2; WIDTH * HEIGHT] };
            for i in 0..WIDTH * HEIGHT {
                let level = if (i % WIDTH + i / WIDTH) % 2 == 0 { 40 } else { 215 };
                f.rgb.extend([level; 3]);
            }
            f
        });
        let camera = std::array::from_fn(|f| {
            let [fx, fy, cx, cy] = self.intrinsics[f];
            CameraFrame { fx, fy, cx, cy, r: self.r[f], t: self.t[f], ts_us: self.ts[f], track: 2 }
        });
        let imu = ImuEvidence { g0: self.g0, a_peak_mg: 0, windows: self.windows.clone() };
        Ok(view_from_parts(&frames, &camera, &imu, &synthetic::synthetic_challenge(seed))?)
    }
}

const PLANE_SEED: u64 = 7;

fn plane(policy: &Policy, rig: &Rig) -> Result<(GeometryView, PolicyReport), FixtureError> {
    annotate(rig.view(PLANE_SEED)?, policy)
}

fn plane_fixtures(policy: &Policy, out: &mut Vec<Fixture>) -> Result<(), FixtureError> {
    let usable_per_frame = |v: &GeometryView, frame: u8| v.pairs.iter().filter(|p| p.frame == frame && p.inb).count() as u16;

    // dcons lower side at equality (frame 0 sees the plane exactly 30 mm farther
    // at d = 1000 mm) and occlusion at equality (frame t+ depth 950 = 0.95·1000).
    let mut rig = Rig::new();
    rig.t[0] = [-4000, 0, -30_000];
    rig.depth[2] = 950;
    let (view, r) = plane(policy, &rig)?;
    let n0 = usable_per_frame(&view, 0);
    ensure(n0 > 0 && r.stats.n_dok == n0 && r.stats.n_occ == 0 && r.stats.n_usable > n0, "plane-depth-boundary-equal")?;
    out.push(fixture("plane-depth-boundary-equal",
        "Fronto-parallel plane, identity rotations. Frame t-: 1000·|dj·M − Q_z| = dcons_pm·dj·M exactly (depth_ok true at equality). Frame t+: 1000·dj·M = (1000 − occ_pm)·Q_z exactly (not occluded, strict inequality; depth_ok false).", view));

    rig.t[0] = [-4000, 0, -30_001];
    rig.depth[2] = 949;
    let (view, r) = plane(policy, &rig)?;
    let n2 = usable_per_frame(&view, 2);
    ensure(n2 > 0 && r.stats.n_dok == 0 && r.stats.n_occ == n2, "plane-depth-boundary-over")?;
    out.push(fixture("plane-depth-boundary-over",
        "As plane-depth-boundary-equal one unit past each boundary: frame t- depth_ok false (30001 > 30000), frame t+ occluded (dj = 949).", view));

    let mut rig = Rig::new();
    rig.t[0] = [-4000, 0, 30_000];
    let (view, r) = plane(policy, &rig)?;
    let n0 = usable_per_frame(&view, 0);
    ensure(n0 > 0 && r.stats.n_dok == r.stats.n_usable && r.stats.n_occ == 0, "plane-depth-boundary-upper")?;
    out.push(fixture("plane-depth-boundary-upper",
        "Frame t- 30 mm closer: dj·M − Q_z = +dcons_pm·dj·M/1000 exactly (upper side of the depth tolerance, depth_ok true).", view));

    // Q_z = 0 for every frame t- pair, Q_z < 0 for every frame t+ pair.
    let mut rig = Rig::new();
    rig.r[0] = TURNED;
    rig.r[2] = TURNED;
    rig.t[0] = [0, 0, 1_000_000];
    rig.t[2] = [0; 3];
    let (view, r) = plane(policy, &rig)?;
    ensure(view.pairs.iter().all(|p| !p.inb && p.uj.is_none() && p.vj.is_none()) && r.stats.n_usable == 0
        && view.pr_bx.iter().chain(&view.pr_by).all(|&x| x == 0), "plane-qz-nonpositive")?;
    out.push(fixture("plane-qz-nonpositive",
        "Frames t-/t+ rotated 180° about y. Frame t-: Q_z = 0 exactly (T_01z = 1000·d0); frame t+: Q_z < 0. Every pair zpos false (uj/vj null), opened block (0,0).", view));

    let mut rig = Rig::new();
    rig.t[0] = [600_000, 400_000, 0];
    rig.t[2] = [-600_000, -400_000, 0];
    let (view, r) = plane(policy, &rig)?;
    let outside = view.pairs.iter().filter(|p| !p.inb && p.uj.is_some()).count();
    ensure(outside > 0 && outside < view.pairs.len() && r.stats.n_usable > 0, "plane-out-of-bounds")?;
    out.push(fixture("plane-out-of-bounds",
        "Lateral translation of 0.6 m / 0.4 m: projections shift by about 126 / 84 px, so some pairs are zpos but outside the raster (negative and ≥ 256/192 coordinates) and some inside.", view));

    // Gyro tolerance at equality: vee(R_01) = 0 and |2^11·Θc_0,0| = 2·gyro_tol_urad·2^30.
    let gyro = |extra: i32| -> Vec<ImuSample> {
        let target = 2 * policy.motion.gyro_tol_urad * Q30 / ((1 << 11) * 16_384);
        let target = i32::try_from(target).expect("v1m gyro tolerance fits i32") + extra;
        let mut rates = [target / 9; 9];
        rates[8] += target - 9 * (target / 9);
        window(&rates.map(|w| (16_384, [0, w, 0])))
    };
    let mut rig = Rig::new();
    rig.windows[0] = gyro(0);
    let (view, r) = plane(policy, &rig)?;
    ensure(r.motion_ok && r.quality_ok, "plane-gyro-boundary-equal")?;
    out.push(fixture("plane-gyro-boundary-equal",
        "Window 0: nine samples of dt 16384 µs with device-y rates summing to exactly 2·gyro_tol_urad·2^30 / (2^11·16384); |vee·10^6 − 2^11·Θc| equals the tolerance (motion_ok true).", view));
    rig.windows[0] = gyro(1);
    let (view, r) = plane(policy, &rig)?;
    ensure(!r.motion_ok && r.quality_ok, "plane-gyro-boundary-over")?;
    out.push(fixture("plane-gyro-boundary-over", "As plane-gyro-boundary-equal with one more rate unit: motion_ok false.", view));

    let mut rig = Rig::new();
    let mut gap = vec![(15_000, [0; 3]); 5];
    gap.push((0, [0; 3]));
    gap.extend([(15_000, [0; 3]); 5]);
    rig.windows[0] = window(&gap);
    let (view, r) = plane(policy, &rig)?;
    ensure(!r.motion_ok, "plane-imu-padding-gap")?;
    out.push(fixture("plane-imu-padding-gap", "Window 0 has a zero-dt sample followed by non-zero samples (padding not at the end): motion_ok false.", view));

    let mut rig = Rig::new();
    let mut samples = rig.windows[1].clone();
    samples[20].w = [0, 0, 1];
    rig.windows[1] = samples;
    let (view, r) = plane(policy, &rig)?;
    ensure(!r.motion_ok, "plane-imu-padding-rate")?;
    out.push(fixture("plane-imu-padding-rate", "Window 1 padding sample 20 has dt 0 but ω_z = 1: motion_ok false.", view));

    let mut rig = Rig::new();
    let full: Vec<(u32, [i32; 3])> = (0..64).map(|k| (if k < 48 { 2344 } else { 2343 }, [0; 3])).collect();
    rig.windows[0] = window(&full);
    rig.windows[1] = window(&[(2381, [0; 3]); 63]);
    let (view, r) = plane(policy, &rig)?;
    ensure(r.motion_ok && view.imu_dt[0][63] > 0 && view.imu_dt[1][62] > 0 && view.imu_dt[1][63] == 0, "plane-imu-full-window")?;
    out.push(fixture("plane-imu-full-window",
        "Window 0 uses all 64 samples (no padding; k = 63 has no successor), window 1 has 63 samples and one padding slot: motion_ok true.", view));

    let q = &policy.quality;
    let mut rig = Rig::new();
    rig.intrinsics[0] = [q.fx_min_q16, q.fx_max_q16, q.cx_min_q16, q.cy_max_q16];
    rig.intrinsics[2] = [q.fx_max_q16, q.fx_min_q16, q.cx_max_q16, q.cy_min_q16];
    let (dt_min, dt_max) = (q.dt_min_us as u64, q.dt_max_us as u64);
    rig.ts = [1_000_000, 1_000_000 + dt_min, 1_000_000 + dt_min + dt_max];
    rig.windows = [window(&[(10_000, [0; 3]); 8]), window(&[(25_000, [0; 3]); 24])];
    let (view, r) = plane(policy, &rig)?;
    ensure(r.quality_ok && r.motion_ok, "plane-quality-bounds-closed")?;
    out.push(fixture("plane-quality-bounds-closed",
        "fx, fy (fy uses the fx bounds), cx, cy and both frame intervals exactly at their closed-interval limits: quality_ok true.", view));
    rig.intrinsics[2][1] = q.fx_max_q16 + 1;
    let (view, r) = plane(policy, &rig)?;
    ensure(!r.quality_ok && r.motion_ok, "plane-quality-bounds-over")?;
    out.push(fixture("plane-quality-bounds-over", "As plane-quality-bounds-closed with fy of frame t+ = fx_max_q16 + 1: quality_ok false.", view));
    Ok(())
}

/// Views at the §20 input range limits: maximum |R|, |T|, intrinsics, ts,
/// dt, ω, g0, a_peak; depth 65535 on invalid pixels and confidence 3.
fn extreme_fixtures(policy: &Policy, out: &mut Vec<Fixture>) -> Result<(), FixtureError> {
    const PIXELS: usize = WIDTH * HEIGHT;
    let frames = |bright: bool| -> [FrameData; 3] {
        std::array::from_fn(|_| FrameData {
            rgb: (0..3 * PIXELS).map(|i| if ((i / 3) % 2 == 0) == bright { 255 } else { 0 }).collect(),
            depth_mm: (0..PIXELS).map(|i| match (i % WIDTH + i / WIDTH) % 3 { 0 => 65535, 1 => 5000, _ => 200 }).collect(),
            conf: vec![3; PIXELS],
        })
    };
    let max26 = (1 << 26) - 1;
    let (lo, hi) = (i64::from(i32::MIN), i64::from(i32::MAX));
    let camera_max: [CameraFrame; 3] = std::array::from_fn(|f| CameraFrame {
        fx: max26, fy: max26, cx: max26, cy: max26,
        r: [hi; 9], t: if f == 1 { [hi; 3] } else { [lo; 3] },
        ts_us: [0, 1 << 47, (1 << 48) - 1][f], track: 3,
    });
    let imu_max = ImuEvidence {
        g0: [i32::MAX; 3], a_peak_mg: u32::MAX,
        windows: [0, 1].map(|_| vec![ImuSample { dt_us: u32::MAX, w: [i32::MAX; 3] }; IMU_MAX]),
    };
    let camera_min: [CameraFrame; 3] = std::array::from_fn(|f| CameraFrame {
        fx: 1, fy: 1, cx: max26, cy: max26,
        r: std::array::from_fn(|i| if (i + f) % 2 == 0 { lo } else { hi }),
        t: if f == 1 { [lo; 3] } else { [hi; 3] },
        ts_us: [(1 << 48) - 1, 1 << 47, 0][f], track: 0,
    });
    let imu_min = ImuEvidence {
        g0: [i32::MIN; 3], a_peak_mg: 0,
        windows: [0, 1].map(|w| (0..IMU_MAX).map(|k| ImuSample {
            dt_us: if w == 0 && k % 2 == 0 { 0 } else { u32::MAX }, w: [i32::MIN; 3],
        }).collect()),
    };
    let challenge = synthetic::synthetic_challenge(PLANE_SEED);
    for (name, target, frames, camera, imu) in [
        ("extreme-max", "Every signed input at i32::MAX (R, T of t0; T of t-/t+ at i32::MIN), fx..cy = 2^26 − 1, ts up to 2^48 − 1, track 3, dt = 2^32 − 1, ω = g0 = i32::MAX, a_peak = 2^32 − 1; depth 65535 (invalid) / 5000 / 200 with confidence 3. Checks gadget bit widths at the documented maxima.",
            frames(true), camera_max, imu_max),
        ("extreme-min", "Negative extremes: R alternating i32::MIN / i32::MAX, T of t0 at i32::MIN and of t-/t+ at i32::MAX, fx = fy = 1, cx = cy = 2^26 − 1, decreasing timestamps, ω = g0 = i32::MIN (also on zero-dt samples), same depth pattern as extreme-max.",
            frames(false), camera_min, imu_min),
    ] {
        let (view, r) = annotate(view_from_parts(&frames, &camera, &imu, &challenge)?, policy)?;
        ensure(!r.pass, "extreme")?;
        out.push(fixture(name, target, view));
    }
    Ok(())
}

/// Test-only policy exercising the §8.2 coefficient bound: v1m with
/// `d_min_mm = 1` and its own name (hence its own policy id).
pub fn coefficient_policy(v1m: &Policy) -> Policy {
    let mut p = v1m.clone();
    p.name = COEFFICIENT_POLICY_NAME.to_owned();
    p.lidar.d_min_mm = 1;
    p
}

/// Room view whose challenge-derived hypothesis 0 is valid under the
/// test-only policy, passes the area check, and exceeds the 2^36 bound.
pub fn coefficient_fixture(test_policy: &Policy) -> Result<Fixture, FixtureError> {
    const NAME: &str = "coefficient-overflow";
    for seed in 0..SEARCH_SEEDS {
        let base: Parts = synthetic::capture(Scenario::Room, seed, &test_policy.motion.m_cd);
        let challenge: Challenge = synthetic::synthetic_challenge(seed);
        let probe = view_from_parts(&base.0, &base.1, &base.2, &challenge)?;
        for near in 0..3 {
            let mut parts = base.clone();
            for (m, &i) in probe.hyp_idx[0].iter().enumerate() {
                let px = point_pixel(&probe, usize::from(i));
                parts.0[1].depth_mm[px] = if m == near { 1 } else { 2500 };
                parts.0[1].conf[px] = 2;
            }
            let view = view_from_parts(&parts.0, &parts.1, &parts.2, &challenge)?;
            if hypothesis_fits(&view, &test_policy.lidar)[0] == Fit::CoefficientBound {
                let (view, _) = annotate(view, test_policy)?;
                return Ok(fixture("view", format!(
                    "Test-only policy {COEFFICIENT_POLICY_NAME} (d_min_mm 1). Room seed {seed}; the three challenge-derived vertices of hypothesis 0 set to depths 2500/2500/1 mm (vertex {near} at 1 mm): valid, |Δ| ≥ area_min, coefficient magnitude ≥ 2^36, so deg_0 = 1."), view));
            }
        }
    }
    Err(FixtureError::Target(NAME))
}

/// Canonical file bytes: compact JSON and a trailing newline.
pub fn to_json(view: &GeometryView) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(view).expect("GeometryView serializes");
    bytes.push(b'\n');
    bytes
}

fn flag(b: bool) -> &'static str { if b { "1" } else { "0" } }

/// README for `test-vectors/policy/`, generated from the same fixture list.
pub fn readme(fixtures: &[Fixture], coefficient: &Fixture, policy_id: &str, test_policy_id: &str) -> String {
    let mut s = String::from("# Policy reference fixtures (galaxy-geometry-view/v1)\n\n\
Generated by `geometry-tool fixtures --out test-vectors/policy` (crate `galaxy-geometry`, module `fixtures`); do not edit by hand. \
Each file is one GeometryView (spec §20) whose `expected` object is the reference engine's §8.8 report under the policy noted below. \
Consumed by the circuit differential tests (`circuits/tests/geometry/policy.test.mjs`) and checked by `cargo test -p galaxy-geometry --test policy_fixtures`.\n\n\
All sensor data is synthetic (ray-cast scenes, `synthetic-*`/`room-*`) or hand-constructed (`plane-*`, `extreme-*`); none comes from a device. \
`room-*` fixtures start from the passing synthetic room capture and change one input. `hyp_idx` is challenge-derived except where a description says it is overridden. \
`uj`/`vj` in `pairs` are metadata (decimal strings, null when Q_z ≤ 0).\n\n");
    s.push_str(&format!("Default-policy fixtures (top-level `*.json`): policy `galaxy-pc-v1m`, policy_id `{policy_id}`.\n\n"));
    s.push_str("| file | quality | geometry | rgb_depth | temporal | motion | pass | target |\n|---|---|---|---|---|---|---|---|\n");
    let row = |name: &str, f: &Fixture| {
        let r = f.view.expected.expect("fixtures carry expected");
        format!("| `{name}` | {} | {} | {} | {} | {} | {} | {} |\n", flag(r.quality_ok), flag(r.geometry_ok), flag(r.rgb_depth_ok),
            flag(r.temporal_ok), flag(r.motion_ok), flag(r.pass), f.target)
    };
    for f in fixtures {
        s.push_str(&row(&format!("{}.json", f.name), f));
    }
    s.push_str(&format!("\nIsolated test-only profile (`{COEFFICIENT_DIR}/`): `policy-test-dmin1.json` is policy `{COEFFICIENT_POLICY_NAME}` (policy_id `{test_policy_id}`), \
identical to galaxy-pc-v1m except `d_min_mm = 1`, so that the §8.2 coefficient bound (unreachable under v1m) can be exercised. \
It is never placed in `policies/` and no configuration accepts it.\n\n"));
    s.push_str("| file | quality | geometry | rgb_depth | temporal | motion | pass | target |\n|---|---|---|---|---|---|---|---|\n");
    s.push_str(&row(&format!("{COEFFICIENT_DIR}/view.json"), coefficient));
    s
}

fn hex_id(policy: &Policy) -> Result<String, FixtureError> {
    Ok(policy.id()?.iter().map(|b| format!("{b:02x}")).collect())
}

/// Every file of `test-vectors/policy/` as (relative path, bytes), in a
/// deterministic order: default fixtures, the isolated coefficient-bound
/// profile (policy + view), and the README.
pub fn render_files(v1m: &Policy) -> Result<Vec<(String, Vec<u8>)>, FixtureError> {
    v1m.validate()?;
    let fixtures = reference_fixtures(v1m)?;
    let test_policy = coefficient_policy(v1m);
    let coefficient = coefficient_fixture(&test_policy)?;
    let mut files: Vec<(String, Vec<u8>)> = fixtures.iter().map(|f| (format!("{}.json", f.name), to_json(&f.view))).collect();
    let mut policy_json = test_policy.canonical_json()?;
    policy_json.push(b'\n');
    files.push((format!("{COEFFICIENT_DIR}/policy-test-dmin1.json"), policy_json));
    files.push((format!("{COEFFICIENT_DIR}/view.json"), to_json(&coefficient.view)));
    let text = readme(&fixtures, &coefficient, &hex_id(v1m)?, &hex_id(&test_policy)?);
    files.push(("README.md".to_owned(), text.into_bytes()));
    Ok(files)
}
