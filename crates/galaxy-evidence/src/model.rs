use ark_bn254::Fr;
use serde::{Deserialize, Serialize};

use crate::ImageProfile;

pub const IMU_MAX: usize = 64;
pub const CAMERA_ELEMENTS: usize = 54;
pub const IMU_ELEMENTS: usize = 5 + 2 * IMU_MAX;
pub const BLUETOOTH_ELEMENTS: usize = 24;
pub const NOISE_ELEMENTS: usize = 18;
pub const META_ELEMENTS: usize = 9;

#[derive(Clone, PartialEq, Eq)]
pub struct FrameData {
    pub rgb: Vec<u8>,
    pub depth_mm: Vec<u16>,
    pub conf: Vec<u8>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct CameraFrame {
    pub fx: i64,
    pub fy: i64,
    pub cx: i64,
    pub cy: i64,
    pub r: [i64; 9],
    pub t: [i64; 3],
    pub ts_us: u64,
    pub track: u8,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct ImuSample {
    pub dt_us: u32,
    pub w: [i32; 3],
}

#[derive(Clone, PartialEq, Eq)]
pub struct ImuEvidence {
    pub g0: [i32; 3],
    pub a_peak_mg: u32,
    pub windows: [Vec<ImuSample>; 2],
}

#[derive(Clone, PartialEq, Eq)]
pub struct CanonicalImage {
    pub profile: ImageProfile,
    pub rgb: Vec<u8>,
    pub encoded: Vec<u8>,
}

/// Counts and bounded RSSI summaries only; never contains peripheral identifiers.
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BluetoothAggregate {
    pub version: u8,
    pub flags: u8,
    pub scan_duration_ms: u32,
    pub n_adv: u32,
    pub n_unique: u32,
    pub rssi_histogram: [u32; 16],
    pub rssi_mean_q8: i32,
    pub rssi_var_q8: u32,
    pub persistence_pct: u8,
}

/// A bounded list of residual summaries, never a persistent sensor fingerprint.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoiseAggregate {
    pub version: u8,
    pub stats: Vec<i16>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Meta {
    pub version: u8,
    pub image_profile: ImageProfile,
    pub app_build: u32,
    pub device_model_code: u32,
    pub frame_offsets_ms: [i32; 3],
    pub ycbcr_matrix_code: u8,
    pub quality_flags: u32,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Evidence {
    pub image: CanonicalImage,
    pub frames: [FrameData; 3],
    pub camera: [CameraFrame; 3],
    pub imu: ImuEvidence,
    pub bluetooth: BluetoothAggregate,
    pub noise: NoiseAggregate,
    pub meta: Meta,
}

/// Independent salts for the eight private component slots. Intentionally not `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct Salts {
    pub depth0: Fr,
    pub frame_minus: Fr,
    pub frame_plus: Fr,
    pub camera: Fr,
    pub imu: Fr,
    pub bluetooth: Fr,
    pub noise: Fr,
    pub meta: Fr,
}
