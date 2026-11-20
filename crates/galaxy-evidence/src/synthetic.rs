//! Deterministic synthetic GXRW v1 captures for tests and golden vectors (spec §21).
//!
//! The scene is a textured, slightly tilted plane about 1.5 m in front of a camera that
//! translates sideways between frames. Every byte is a pure function of [`SyntheticCapture`];
//! no randomness or clock is read. This is fixture data, never a substitute for a device capture.

use galaxy_image::{ANALYSIS_HEIGHT, ANALYSIS_WIDTH, SOURCE_HEIGHT, SOURCE_WIDTH};
use serde_json::{json, Value};

const Y_LEN: usize = SOURCE_WIDTH * SOURCE_HEIGHT;
const DEPTH_PIXELS: usize = ANALYSIS_WIDTH * ANALYSIS_HEIGHT;

#[derive(Clone, Debug)]
pub struct SyntheticCapture {
    /// Seed of the plane texture.
    pub texture_seed: u64,
    /// ARFrame timestamps (s) of the minus, center and plus frames.
    pub frame_times_s: [f64; 3],
    /// Camera x translation (m) per frame, in ARKit world coordinates.
    pub camera_x_m: [f64; 3],
    /// Plane distance (m) at the image center.
    pub depth_m: f64,
    /// IMU sample period (s) and the margin (s) covered before the first and after the last frame.
    pub imu_period_s: f64,
    pub imu_margin_s: f64,
    /// Constant rotation rate (rad/s) reported by every IMU sample.
    pub rotation_rate: [f64; 3],
}

impl Default for SyntheticCapture {
    fn default() -> Self {
        Self {
            texture_seed: 0x6761_6c61_7879_0001,
            frame_times_s: [5000.0, 5000.15, 5000.3],
            camera_x_m: [-0.02, 0.0, 0.02],
            depth_m: 1.5,
            imu_period_s: 0.01,
            imu_margin_s: 0.08,
            rotation_rate: [0.002, -0.001, 0.0005],
        }
    }
}

fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Region descriptor as written in the header.
fn region(offset: usize, len: usize) -> Value {
    json!({ "offset": offset, "len": len })
}

impl SyntheticCapture {
    /// Horizontal image shift (source pixels) of frame `f` relative to the center frame.
    fn shift(&self, f: usize) -> i64 {
        let fx = 1440.0;
        (((self.camera_x_m[f] - self.camera_x_m[1]) * fx / self.depth_m).round()) as i64
    }

    fn texture(&self, x: i64, y: i64) -> u8 {
        // Smooth cells with value noise and hard edges every 48 px: enough structure for sampling.
        let cell = mix(self.texture_seed ^ (((x.div_euclid(48)) as u64) << 32) ^ (y.div_euclid(48)) as u64);
        let fine = mix(self.texture_seed.rotate_left(17) ^ ((x as u64) << 32) ^ y as u64);
        let base = 40 + (cell % 160) as i64 + (x.rem_euclid(48) + y.rem_euclid(48)) / 4;
        (base + (fine % 7) as i64 - 3).clamp(0, 255) as u8
    }

    fn planes(&self, f: usize) -> (Vec<u8>, Vec<u8>) {
        let shift = self.shift(f);
        let mut y = Vec::with_capacity(Y_LEN);
        for row in 0..SOURCE_HEIGHT as i64 {
            for col in 0..SOURCE_WIDTH as i64 {
                y.push(self.texture(col + shift, row));
            }
        }
        let mut cbcr = Vec::with_capacity(Y_LEN / 2);
        for row in 0..(SOURCE_HEIGHT / 2) as i64 {
            for col in 0..(SOURCE_WIDTH / 2) as i64 {
                let u = col + shift / 2;
                cbcr.push((96 + u.rem_euclid(640) / 10) as u8);
                cbcr.push((150 - row / 12) as u8);
            }
        }
        (y, cbcr)
    }

    fn depth(&self, f: usize) -> (Vec<u8>, Vec<u8>) {
        let mut depth = Vec::with_capacity(DEPTH_PIXELS * 4);
        let mut conf = Vec::with_capacity(DEPTH_PIXELS);
        for v in 0..ANALYSIS_HEIGHT {
            for u in 0..ANALYSIS_WIDTH {
                let border = u < 2 || v < 2 || u >= ANALYSIS_WIDTH - 2 || v >= ANALYSIS_HEIGHT - 2;
                let value = if border && (u + v + f) % 5 == 0 {
                    f32::NAN
                } else {
                    (self.depth_m + 0.0004 * (u as f64 - 128.0)) as f32
                };
                depth.extend_from_slice(&value.to_le_bytes());
                conf.push(if border { (u % 2) as u8 } else { 2 });
            }
        }
        (depth, conf)
    }

    fn transform(&self, f: usize) -> Vec<f64> {
        // Column-major camera→world with identity rotation (ARKit camera axes).
        let mut m = vec![0.0; 16];
        m[0] = 1.0;
        m[5] = 1.0;
        m[10] = 1.0;
        m[15] = 1.0;
        m[12] = self.camera_x_m[f];
        m[13] = 0.001 * f as f64;
        m[14] = 0.0;
        m
    }

    fn imu_samples(&self) -> Vec<Value> {
        let start = self.frame_times_s[0] - self.imu_margin_s;
        let end = self.frame_times_s[2] + self.imu_margin_s;
        let count = ((end - start) / self.imu_period_s).ceil() as usize + 1;
        (0..count)
            .map(|k| {
                let t = start + k as f64 * self.imu_period_s;
                let [rx, ry, rz] = self.rotation_rate;
                let wobble = 0.01 * ((k % 7) as f64 - 3.0);
                json!([t, rx, ry, rz, 0.0, -0.998, -0.05, wobble, 0.02, -0.01])
            })
            .collect()
    }

    /// The JSON header with payload regions laid out frame by frame (y, cbcr, depth, conf).
    pub fn header(&self) -> Value {
        let roles = ["minus", "center", "plus"];
        let mut offset = 0;
        let frames: Vec<Value> = (0..3)
            .map(|f| {
                let mut next = |len| {
                    let r = region(offset, len);
                    offset += len;
                    r
                };
                let (y, cbcr, depth, conf) = (next(Y_LEN), next(Y_LEN / 2), next(DEPTH_PIXELS * 4), next(DEPTH_PIXELS));
                json!({
                    "role": roles[f],
                    "timestamp_s": self.frame_times_s[f],
                    "intrinsics": [1440.25, 1440.25, 959.5, 719.75],
                    "transform": self.transform(f),
                    "tracking": 2,
                    "tracking_reason": "normal",
                    "y": y, "cbcr": cbcr, "depth": depth, "conf": conf,
                })
            })
            .collect();
        let mut histogram = [0u32; 16];
        histogram[9] = 3;
        histogram[10] = 5;
        histogram[12] = 2;
        json!({
            "format": "galaxy-raw-capture/v1",
            "device": { "model": "iPhone14,3", "os": "26.1", "app_build": "42" },
            "image": { "width": SOURCE_WIDTH, "height": SOURCE_HEIGHT, "pixel_format": "420f", "ycbcr_matrix": "ITU_R_601_4" },
            "depth": { "width": ANALYSIS_WIDTH, "height": ANALYSIS_HEIGHT },
            "frames": frames,
            "imu": { "samples": self.imu_samples() },
            "bluetooth": {
                "version": 1, "flags": 7, "scan_duration_ms": 3000, "n_adv": 10, "n_unique": 4,
                "rssi_histogram": histogram, "rssi_mean_q8": -12_800, "rssi_var_q8": 9_000, "persistence_pct": 75,
            },
            "noise": { "version": 1, "stats": [12, -3, 40, 7, 0, -18, 255, 1] },
            "claimed_time_ms": 1_795_000_000_000u64,
            "quality": { "flags": 0, "reasons": [] },
        })
    }

    /// Payload bytes in the layout described by [`Self::header`].
    pub fn payload(&self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(3 * (Y_LEN * 3 / 2 + DEPTH_PIXELS * 5));
        for f in 0..3 {
            let (y, cbcr) = self.planes(f);
            let (depth, conf) = self.depth(f);
            payload.extend(y);
            payload.extend(cbcr);
            payload.extend(depth);
            payload.extend(conf);
        }
        payload
    }

    pub fn to_gxrw(&self) -> Vec<u8> {
        assemble(&self.header(), &self.payload())
    }
}

/// `"GXRW"` ‖ version 1 ‖ u32be header length ‖ compact JSON header ‖ payload.
pub fn assemble(header: &Value, payload: &[u8]) -> Vec<u8> {
    let header = serde_json::to_vec(header).expect("JSON value serializes");
    let mut out = Vec::with_capacity(9 + header.len() + payload.len());
    out.extend_from_slice(b"GXRW\x01");
    out.extend_from_slice(&(header.len() as u32).to_be_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(payload);
    out
}
