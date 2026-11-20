//! Deterministic development-only captures. These are simulated sensor data,
//! not device evidence or attestations. Floating point is only used to render
//! and quantize synthetic input; all policy calculations remain integer-only.

mod scene;
pub mod suite;

use core::fmt;
use core::str::FromStr;
use galaxy_crypto::sampling::Challenge;
use galaxy_evidence::{CameraFrame, CanonicalImage, Evidence, FrameData, ImageProfile, ImuEvidence, ImuSample, Meta};
use galaxy_protocol::policy::Policy;

use crate::{HEIGHT, IMU_MAX, WIDTH};

const FX: f64 = 210.0;
const FY: f64 = 209.0;
const CX: f64 = 127.5;
const CY: f64 = 95.5;
const WINDOW_US: u32 = 150_000;
const SAMPLE_US: u32 = 10_000;
const Q20: f64 = (1u64 << 20) as f64;
const Q30: f64 = (1u64 << 30) as f64;
const Q16: f64 = (1u64 << 16) as f64;

macro_rules! scenarios {
    ($($variant:ident => $name:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Scenario { $($variant),+ }

        impl Scenario {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $name),+ }
            }
        }
    }
}

scenarios! {
    Room => "room",
    FlatWall => "flat-wall",
    FrontalMonitor => "frontal-monitor",
    TiltedMonitor => "tilted-monitor",
    CurvedMonitor => "curved-monitor",
    CentreMonitor => "centre-monitor",
    Print => "print",
    PureRotation => "pure-rotation",
    Static => "static",
    LowTexture => "low-texture",
    Reflective => "reflective",
    Dynamic => "dynamic",
    DepthSubstitution => "depth-substitution",
    SwappedFrames => "swapped-frames",
    ViewpointSubstitution => "viewpoint-substitution",
    ImuSubstitution => "imu-substitution",
}

impl fmt::Display for Scenario {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(self.as_str()) }
}

impl FromStr for Scenario {
    type Err = UnknownScenario;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL.iter().copied().find(|scenario| scenario.as_str() == s).ok_or(UnknownScenario)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown synthetic scenario")]
pub struct UnknownScenario;

#[derive(Debug, thiserror::Error)]
#[error("cannot encode or decode synthetic image")]
pub struct SyntheticError(#[from] galaxy_image::ImageError);

/// Deterministic, development-only challenge for a synthetic scene. Real
/// captures must pass the actual capture ID and validated selection ledger.
pub fn synthetic_challenge(seed: u64) -> Challenge {
    let mut capture = [0; 32];
    capture[..24].copy_from_slice(b"GALAXY/SYNTHETIC/VIEW/v1");
    capture[24..].copy_from_slice(&seed.to_be_bytes());
    Challenge::derive(1, &capture, 5, &[0x53; 32])
}

/// Ray cast a synthetic capture and produce a lossless canonical image whose
/// canonical samples match the central RGB grid. Bluetooth/noise absent; meta
/// uses device model code zero, never a real device model identifier.
pub fn synthesize(scenario: Scenario, seed: u64, policy: &Policy) -> Result<Evidence, SyntheticError> {
    synthesize_with_profile(scenario, seed, policy, ImageProfile::PngV1)
}

/// For JPEG, replace the raster and central samples with the committed
/// reference decode of the exported bytes, just like device ingest.
pub fn synthesize_with_profile(scenario: Scenario, seed: u64, policy: &Policy, profile: ImageProfile) -> Result<Evidence, SyntheticError> {
    let (mut frames, camera, imu) = capture(scenario, seed, &policy.motion.m_cd);
    let mut rgb = canonical_rgb(&frames[1].rgb);
    let encoded = galaxy_image::encode(profile, &rgb)?;
    if profile == ImageProfile::JpegV1 {
        rgb = galaxy_image::decode_any(&encoded)?.raster;
        frames[1].rgb = galaxy_image::analysis_raster(&rgb)?;
    }
    Ok(Evidence {
        image: CanonicalImage { profile, rgb, encoded }, frames, camera, imu,
        bluetooth: Default::default(), noise: Default::default(),
        meta: Meta {
            version: 1, image_profile: profile, app_build: 0, device_model_code: 0,
            frame_offsets_ms: [-150, 0, 150], ycbcr_matrix_code: 0, quality_flags: 0,
        },
    })
}

fn canonical_rgb(analysis: &[u8]) -> Vec<u8> {
    let mut rgb = vec![0; WIDTH * HEIGHT * 16 * 3];
    for y in 0..HEIGHT * 4 {
        for x in 0..WIDTH * 4 {
            let a = 3 * ((y / 4) * WIDTH + x / 4);
            let b = 3 * (y * WIDTH * 4 + x);
            rgb[b..b + 3].copy_from_slice(&analysis[a..a + 3]);
        }
    }
    rgb
}

pub(crate) fn capture(scenario: Scenario, seed: u64, m_cd: &[i64; 9]) -> ([FrameData; 3], [CameraFrame; 3], ImuEvidence) {
    let mut rng = Rng(seed ^ 0x366e_b3a8_ec26_6d6d);
    let poses = trajectory(scenario, &mut rng);
    let seed_scene = rng.next();
    let mut frames = std::array::from_fn(|f| scene::render(scenario, seed_scene, poses[f], f));
    match scenario {
        Scenario::DepthSubstitution => {
            for (f, frame) in frames.iter_mut().enumerate() {
                let alt = scene::render(Scenario::Room, seed_scene ^ 0x9814_95aa, poses[f], f);
                frame.depth_mm = alt.depth_mm;
                frame.conf = alt.conf;
            }
        }
        Scenario::SwappedFrames => frames.swap(0, 2),
        Scenario::ViewpointSubstitution => {
            let pose = Pose { t: [0.42, -0.25, 0.05], angles: [0.04, -0.08, -0.04] };
            frames[1].rgb = scene::render(Scenario::Room, seed_scene, pose, 1).rgb;
        }
        _ => {}
    }
    let camera = std::array::from_fn(|f| {
        let pose = poses[f];
        let cv_to_world = pose.matrix();
        // Rendering world has y down/z forward. Convert it to the ARKit
        // world convention by the proper rotation diag(1,-1,-1).
        let r = std::array::from_fn(|k| q(cv_to_world[k] * world_sign(k / 3), Q30));
        let t = std::array::from_fn(|i| q(pose.t[i] * world_sign(i), 1e6));
        CameraFrame {
            fx: q(FX, Q16), fy: q(FY, Q16), cx: q(CX, Q16), cy: q(CY, Q16), r, t,
            ts_us: 1_000_000 + f as u64 * u64::from(WINDOW_US), track: 2,
        }
    });
    let imu_poses = if scenario == Scenario::ImuSubstitution {
        trajectory(Scenario::Room, &mut rng)
    } else {
        poses
    };
    (frames, camera, imu(imu_poses, m_cd, &mut rng))
}

fn q(x: f64, scale: f64) -> i64 { libm::round(x * scale) as i64 }
fn world_sign(i: usize) -> f64 { if i == 0 { 1.0 } else { -1.0 } }

#[derive(Clone, Copy, Default)]
struct Pose {
    t: [f64; 3],
    angles: [f64; 3],
}

impl Pose {
    fn matrix(self) -> [f64; 9] {
        let [a, b, c] = self.angles;
        let (s, x) = libm::sincos(a);
        let (t, y) = libm::sincos(b);
        let (u, z) = libm::sincos(c);
        [z*y, z*t*s-u*x, z*t*x+u*s, u*y, u*t*s+z*x, u*t*x-z*s, -t, y*s, y*x]
    }
}

fn vec_len(w: [f64; 3]) -> f64 {
    libm::sqrt(w.into_iter().map(|x| x * x).sum())
}

fn trajectory(scenario: Scenario, rng: &mut Rng) -> [Pose; 3] {
    let unit = |rng: &mut Rng| {
        let mut v = [rng.signed(), rng.signed(), rng.signed()];
        let norm = vec_len(v);
        if norm < 1e-10 { return [1.0, 0.0, 0.0] }
        for x in &mut v { *x /= norm; }
        v
    };
    let center = Pose { t: [0.0; 3], angles: [0.06 * rng.signed(), 0.06 * rng.signed(), 0.12 * rng.signed()] };
    let move_dir = unit(rng);
    let rot_dir = unit(rng);
    let mut step = |sign: f64| {
        let scale = if matches!(scenario, Scenario::Static | Scenario::PureRotation) { 0.0 } else { 0.002 + 0.013 * rng.unit() };
        let spin = if scenario == Scenario::Static { 0.0 } else { 0.015 + 0.050 * rng.unit() };
        Pose {
            t: move_dir.map(|x| sign * scale * x),
            angles: std::array::from_fn(|a| center.angles[a] + sign * spin * rot_dir[a]),
        }
    };
    [step(-1.0), center, step(1.0)]
}

fn imu(poses: [Pose; 3], m_cd: &[i64; 9], rng: &mut Rng) -> ImuEvidence {
    let mats = poses.map(Pose::matrix);
    let transpose_mul = |v: [f64; 3]| -> [f64; 3] {
        std::array::from_fn(|a| (0..3).map(|k| m_cd[k * 3 + a] as f64 * v[k]).sum())
    };
    let gravity = transpose_mul([mats[1][3], mats[1][4], mats[1][5]]);
    let g0 = gravity.map(|x| q(x + 0.0005 * rng.normal(), Q20) as i32);
    let bias: [f64; 3] = std::array::from_fn(|_| 0.0015 * rng.normal());
    let mut max_acc: f64 = 0.0;
    let windows = [0, 1].map(|w| {
        let rel = |a: usize, b: usize| (0..3).map(|k| mats[w][k * 3 + a] * mats[w + 1][k * 3 + b]).sum::<f64>();
        let skew = [rel(2, 1) - rel(1, 2), rel(0, 2) - rel(2, 0), rel(1, 0) - rel(0, 1)];
        let sin_angle = (vec_len(skew) * 0.5).min(1.0);
        let ratio = if sin_angle > 1e-10 { libm::asin(sin_angle) / sin_angle } else { 1.0 };
        let rate = transpose_mul(skew.map(|x| x * ratio / (2.0 * f64::from(WINDOW_US) * 1e-6)));
        (0..IMU_MAX).map(|k| {
            if k >= (WINDOW_US / SAMPLE_US) as usize { return ImuSample::default() }
            let acc = [0.025 * rng.normal(), 0.025 * rng.normal(), 0.025 * rng.normal()];
            max_acc = max_acc.max(vec_len(acc));
            ImuSample { dt_us: SAMPLE_US, w: std::array::from_fn(|i| q(rate[i] + bias[i] + 0.003 * rng.normal(), Q20) as i32) }
        }).collect()
    });
    ImuEvidence { g0, a_peak_mg: q(max_acc, 1000.0) as u32, windows }
}

// SplitMix64 (deterministic test data, not cryptographic randomness).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut x = self.0;
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        x ^ (x >> 31)
    }
    fn unit(&mut self) -> f64 { ((self.next() >> 11) as f64) * (1.0 / ((1u64 << 53) as f64)) }
    fn signed(&mut self) -> f64 { 2.0 * self.unit() - 1.0 }
    fn normal(&mut self) -> f64 {
        // Six uniform variates, centered and scaled to unit variance.
        ((0..6).map(|_| self.unit()).sum::<f64>() - 3.0) * core::f64::consts::SQRT_2
    }
}
