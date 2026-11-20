//! GXRW v1 reader and deterministic ARKit quantization. No raw samples are logged.

use galaxy_image::{
    analysis_raster, decode_any, encode, ycbcr420_full_to_canonical, ImageError, ANALYSIS_HEIGHT,
    ANALYSIS_WIDTH, SOURCE_HEIGHT, SOURCE_WIDTH,
};
use serde::Deserialize;
use thiserror::Error;

use crate::{
    validation::offset_ms, BluetoothAggregate, CameraFrame, CanonicalImage, Evidence, FrameData,
    ImageProfile, ImuEvidence, ImuSample, Meta, NoiseAggregate, ValidationError, IMU_MAX,
};

const HEADER_MAX: usize = 1024 * 1024;
const N: usize = ANALYSIS_WIDTH * ANALYSIS_HEIGHT;
const Y_LEN: usize = SOURCE_WIDTH * SOURCE_HEIGHT;
const REGION_SIZES: [usize; 4] = [Y_LEN, Y_LEN / 2, N * 4, N];
const PAYLOAD_LEN: usize = 3 * (Y_LEN + Y_LEN / 2 + N * 5);

#[derive(Debug, Error)]
pub enum IngestError {
    #[error("invalid GXRW prefix or header length")]
    Prefix,
    #[error("invalid GXRW header (line {line}, column {column})")]
    Header { line: usize, column: usize },
    #[error("unsupported GXRW capture format or dimensions")]
    Format,
    #[error("invalid GXRW payload coverage or region size")]
    Regions,
    #[error("invalid frame order or frame timestamp")]
    Frames,
    #[error("invalid quantized {0}")]
    Quantization(&'static str),
    #[error("invalid IMU sample coverage or window size")]
    Motion,
    #[error(transparent)]
    Evidence(#[from] ValidationError),
    #[error(transparent)]
    Image(#[from] ImageError),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    format: String,
    device: Device,
    image: Image,
    depth: Dimensions,
    frames: [Frame; 3],
    imu: Motion,
    #[serde(default)]
    bluetooth: BluetoothAggregate,
    #[serde(default)]
    noise: NoiseAggregate,
    #[serde(rename = "claimed_time_ms")]
    _claimed_time_ms: u64,
    quality: Quality,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Device {
    model: String,
    #[serde(rename = "os")]
    _os: String,
    app_build: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Image {
    width: usize,
    height: usize,
    pixel_format: String,
    ycbcr_matrix: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Dimensions {
    width: usize,
    height: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Quality {
    flags: u32,
    #[serde(rename = "reasons")]
    _reasons: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Motion {
    samples: Vec<[f64; 10]>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    role: String,
    timestamp_s: f64,
    intrinsics: [f64; 4],
    transform: [f64; 16],
    tracking: u8,
    #[serde(rename = "tracking_reason")]
    _tracking_reason: String,
    y: Region,
    cbcr: Region,
    depth: Region,
    conf: Region,
}

#[derive(Copy, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Region {
    offset: u64,
    len: u64,
}

impl Region {
    fn bytes(self, payload: &[u8]) -> &[u8] {
        // Only used after all regions have been validated against this payload.
        &payload[self.offset as usize..(self.offset + self.len) as usize]
    }
}

impl Frame {
    fn regions(&self) -> [Region; 4] {
        [self.y, self.cbcr, self.depth, self.conf]
    }
}

fn read_header(bytes: &[u8]) -> Result<(Header, &[u8]), IngestError> {
    let rest = bytes.strip_prefix(b"GXRW\x01").ok_or(IngestError::Prefix)?;
    let (size, rest) = rest.split_first_chunk::<4>().ok_or(IngestError::Prefix)?;
    let size = u32::from_be_bytes(*size) as usize;
    if size == 0 || size > HEADER_MAX {
        return Err(IngestError::Prefix);
    }
    let (header, payload) = rest.split_at_checked(size).ok_or(IngestError::Prefix)?;
    if payload.len() != PAYLOAD_LEN {
        return Err(IngestError::Regions);
    }
    let header = serde_json::from_slice(header).map_err(|error| IngestError::Header {
        line: error.line(),
        column: error.column(),
    })?;
    Ok((header, payload))
}

fn validate_regions(header: &Header, payload: &[u8]) -> Result<(), IngestError> {
    let mut spans = [(0u64, 0u64); 12];
    for (frame, spans) in header.frames.iter().zip(spans.chunks_exact_mut(4)) {
        for ((r, span), expected) in frame.regions().into_iter().zip(spans).zip(REGION_SIZES) {
            if r.len != expected as u64 {
                return Err(IngestError::Regions);
            }
            let end = r.offset.checked_add(r.len).ok_or(IngestError::Regions)?;
            if end > payload.len() as u64 {
                return Err(IngestError::Regions);
            }
            *span = (r.offset, end);
        }
    }
    spans.sort_unstable();
    let mut offset = 0;
    for (start, end) in spans {
        if start != offset {
            return Err(IngestError::Regions);
        }
        offset = end;
    }
    if offset != payload.len() as u64 {
        return Err(IngestError::Regions);
    }
    Ok(())
}

fn quant(value: f64, min: f64, max: f64, label: &'static str) -> Result<i64, IngestError> {
    let rounded = value.round();
    if !rounded.is_finite() || rounded < min || rounded > max {
        return Err(IngestError::Quantization(label));
    }
    Ok(rounded as i64)
}

fn quant_i32(value: f64, label: &'static str) -> Result<i32, IngestError> {
    Ok(quant(value, f64::from(i32::MIN), f64::from(i32::MAX), label)? as i32)
}

fn timestamp(seconds: f64) -> Result<u64, IngestError> {
    if seconds < 0.0 {
        return Err(IngestError::Quantization("timestamp"));
    }
    Ok(quant(seconds * 1_000_000.0, 0.0, ((1u64 << 48) - 1) as f64, "timestamp")? as u64)
}

fn camera_frame(frame: &Frame) -> Result<CameraFrame, IngestError> {
    if frame.tracking > 2 || frame.transform.iter().any(|v| !v.is_finite()) {
        return Err(IngestError::Quantization("pose or tracking"));
    }
    let [fx, fy, cx, cy] = frame.intrinsics;
    let [fx, fy, cx, cy] = [fx / 7.5, fy / 7.5, (cx + 0.5) / 7.5 - 0.5, (cy + 0.5) / 7.5 - 0.5];
    let intr = |v| quant(v * 65_536.0, 0.0, ((1u32 << 26) - 1) as f64, "intrinsics");
    let mut r = [0; 9];
    for (i, entry) in r.iter_mut().enumerate() {
        let (row, col) = (i / 3, i % 3);
        let sign = if col == 0 { 1.0 } else { -1.0 };
        *entry = i64::from(quant_i32(frame.transform[4 * col + row] * sign * 1_073_741_824.0, "rotation")?);
    }
    let mut t = [0; 3];
    for (entry, value) in t.iter_mut().zip(&frame.transform[12..15]) {
        *entry = i64::from(quant_i32(value * 1_000_000.0, "translation")?);
    }
    Ok(CameraFrame { fx: intr(fx)?, fy: intr(fy)?, cx: intr(cx)?, cy: intr(cy)?, r, t, ts_us: timestamp(frame.timestamp_s)?, track: frame.tracking })
}

fn imu_evidence(mut motion: Motion, camera: &[CameraFrame; 3], frame_times: [f64; 3]) -> Result<ImuEvidence, IngestError> {
    if motion.samples.iter().flatten().any(|v| !v.is_finite()) {
        return Err(IngestError::Quantization("IMU samples"));
    }
    // Stable ordering retains the first input record among equal raw timestamps.
    // For collisions only after rounding, the earlier raw timestamp wins.
    motion.samples.sort_by(|a, b| a[0].partial_cmp(&b[0]).expect("finite timestamps"));
    let [minus, _, plus] = frame_times;
    if motion.samples.first().is_none_or(|s| s[0] > minus - 0.05)
        || motion.samples.last().is_none_or(|s| s[0] < plus + 0.05)
    {
        return Err(IngestError::Motion);
    }
    let [t0, t1, t2] = camera.map(|c| c.ts_us);
    let mut windows = [Vec::with_capacity(IMU_MAX), Vec::with_capacity(IMU_MAX)];
    let mut previous = [t0, t1];
    let mut prior_timestamp = None;
    let mut gravity = None;
    let mut peak_g2 = 0.0f64;
    for [ts, wx, wy, wz, gx, gy, gz, ax, ay, az] in motion.samples {
        let ts = timestamp(ts)?;
        if prior_timestamp.replace(ts) == Some(ts) {
            continue;
        }
        let omega = |v| quant_i32(v * 1_048_576.0, "angular velocity");
        let w = [omega(wx)?, omega(wy)?, omega(wz)?];
        let g = [quant_i32(gx * 1_048_576.0, "gravity")?, quant_i32(gy * 1_048_576.0, "gravity")?, quant_i32(gz * 1_048_576.0, "gravity")?];
        let delta = ts.abs_diff(t1);
        if gravity.is_none_or(|(best, _)| delta < best) {
            gravity = Some((delta, g));
        }
        if (t0..=t2).contains(&ts) {
            peak_g2 = peak_g2.max((ax * ax + ay * ay) + az * az);
        }
        if ts > t0 && ts <= t2 {
            let i = usize::from(ts > t1);
            if windows[i].len() == IMU_MAX {
                return Err(IngestError::Motion);
            }
            let dt_us = u32::try_from(ts - previous[i]).map_err(|_| IngestError::Quantization("IMU interval"))?;
            windows[i].push(ImuSample { dt_us, w });
            previous[i] = ts;
        }
    }
    for window in &mut windows {
        window.resize(IMU_MAX, ImuSample::default());
    }
    Ok(ImuEvidence {
        g0: gravity.ok_or(IngestError::Motion)?.1,
        a_peak_mg: quant(peak_g2.sqrt() * 1000.0, 0.0, f64::from(u32::MAX), "peak acceleration")? as u32,
        windows,
    })
}

fn depth_mm(value: f32) -> u16 {
    let d = f64::from(value);
    if d.is_finite() && d > 0.0 && d <= 65.535 {
        (d * 1000.0).round_ties_even() as u16
    } else {
        0
    }
}

fn frame_data(frame: &Frame, payload: &[u8], rgb: &[u8]) -> Result<FrameData, IngestError> {
    let conf = frame.conf.bytes(payload);
    if conf.iter().any(|&c| c > 2) {
        return Err(ValidationError::Frame.into());
    }
    let depth_mm = frame.depth.bytes(payload).chunks_exact(4)
        .map(|px| depth_mm(f32::from_le_bytes(px.try_into().expect("depth word"))))
        .collect();
    Ok(FrameData { rgb: analysis_raster(rgb)?, depth_mm, conf: conf.to_vec() })
}

fn device_model(model: &str) -> u32 {
    match model {
        "iPhone13,3" => 1, // 12 Pro
        "iPhone13,4" => 2, // 12 Pro Max
        "iPhone14,2" => 3, // 13 Pro
        "iPhone14,3" => 4, // 13 Pro Max
        "iPhone15,2" => 5, // 14 Pro
        "iPhone15,3" => 6, // 14 Pro Max
        "iPhone16,1" => 7, // 15 Pro
        "iPhone16,2" => 8, // 15 Pro Max
        "iPhone17,1" => 9, // 16 Pro
        "iPhone17,2" => 10, // 16 Pro Max
        _ => 0,
    }
}

fn matrix_code(attachment: &str) -> u8 {
    match attachment {
        "ITU_R_601_4" => 1,
        "ITU_R_709_2" => 2,
        "ITU_R_2020" => 3,
        "SMPTE_240M_1995" => 4,
        _ => 0,
    }
}

pub fn from_gxrw(bytes: &[u8], profile: ImageProfile) -> Result<Evidence, IngestError> {
    let (header, payload) = read_header(bytes)?;
    if header.format != "galaxy-raw-capture/v1"
        || (header.image.width, header.image.height) != (SOURCE_WIDTH, SOURCE_HEIGHT)
        || header.image.pixel_format != "420f"
        || (header.depth.width, header.depth.height) != (ANALYSIS_WIDTH, ANALYSIS_HEIGHT)
    {
        return Err(IngestError::Format);
    }
    validate_regions(&header, payload)?;
    if !header.frames.iter().zip(["minus", "center", "plus"]).all(|(f, role)| f.role == role) {
        return Err(IngestError::Frames);
    }
    let [minus, center, plus] = &header.frames;
    let camera = [camera_frame(minus)?, camera_frame(center)?, camera_frame(plus)?];
    if !camera.windows(2).all(|c| c[0].ts_us < c[1].ts_us) {
        return Err(IngestError::Frames);
    }
    let imu = imu_evidence(header.imu, &camera, [minus.timestamp_s, center.timestamp_s, plus.timestamp_s])?;
    header.bluetooth.validate()?;
    header.noise.validate()?;
    let app_build = &header.device.app_build;
    if app_build.is_empty() || !app_build.bytes().all(|c| c.is_ascii_digit()) {
        return Err(ValidationError::Meta.into());
    }
    let app_build = app_build.parse().map_err(|_| ValidationError::Meta)?;
    let frame_offsets_ms = [offset_ms(camera[0].ts_us, camera[1].ts_us)?, 0, offset_ms(camera[2].ts_us, camera[1].ts_us)?];
    let meta = Meta {
        version: 1,
        image_profile: profile,
        app_build,
        device_model_code: device_model(&header.device.model),
        frame_offsets_ms,
        ycbcr_matrix_code: matrix_code(&header.image.ycbcr_matrix),
        quality_flags: header.quality.flags,
    };
    let raster = |f: &Frame| ycbcr420_full_to_canonical(f.y.bytes(payload), f.cbcr.bytes(payload));
    let rgb = raster(center)?;
    let encoded = encode(profile, &rgb)?;
    let rgb = match profile {
        ImageProfile::PngV1 => rgb,
        ImageProfile::JpegV1 => decode_any(&encoded)?.raster,
    };
    let frames = [frame_data(minus, payload, &raster(minus)?)?, frame_data(center, payload, &rgb)?, frame_data(plus, payload, &raster(plus)?)?];
    let image = CanonicalImage { profile, rgb, encoded };
    let evidence = Evidence { image, frames, camera, imu, bluetooth: header.bluetooth, noise: header.noise, meta };
    evidence.validate()?;
    Ok(evidence)
}
