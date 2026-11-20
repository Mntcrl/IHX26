//! Strict GXEV v1 persistence. Bundle bytes are sensitive and never belong in logs.

use galaxy_crypto::field::{fr_from_be_canonical, fr_to_be};
use galaxy_image::{analysis_raster, decode_any, ImageError, ANALYSIS_HEIGHT, ANALYSIS_WIDTH, MAX_IMAGE_BYTES};
use thiserror::Error;

use crate::{BluetoothAggregate, CameraFrame, CanonicalImage, Evidence, FrameData, ImageProfile, ImuEvidence, ImuSample, Meta, NoiseAggregate, Salts, ValidationError, IMU_MAX};

const MAGIC: &[u8] = b"GXEV\x01";
const PIXELS: usize = ANALYSIS_WIDTH * ANALYSIS_HEIGHT;
pub const MAX_CAPTURE_STATE_BYTES: usize = 1024 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub struct EvidenceBundle {
    pub evidence: Evidence,
    pub salts: Salts,
    pub capture_state: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum BundleError {
    #[error("invalid GXEV version or magic")]
    Prefix,
    #[error("invalid GXEV length or trailing bytes")]
    Length,
    #[error("invalid GXEV profile or embedded raster")]
    Profile,
    #[error("non-canonical GXEV field element")]
    Field,
    #[error(transparent)]
    Evidence(#[from] ValidationError),
    #[error(transparent)]
    Image(#[from] ImageError),
}

fn length(out: &mut Vec<u8>, n: usize) {
    out.extend_from_slice(&(n as u32).to_be_bytes());
}

fn blob(out: &mut Vec<u8>, bytes: &[u8]) {
    length(out, bytes.len());
    out.extend_from_slice(bytes);
}

fn verify_container(encoded: &[u8], profile: ImageProfile) -> Result<Vec<u8>, BundleError> {
    let decoded = decode_any(encoded)?;
    if decoded.profile_hint != profile || decoded.xmp.is_some() {
        return Err(BundleError::Profile);
    }
    Ok(decoded.raster)
}

pub fn encode(bundle: &EvidenceBundle) -> Result<Vec<u8>, BundleError> {
    let e = &bundle.evidence;
    e.validate()?;
    if bundle.capture_state.len() > MAX_CAPTURE_STATE_BYTES {
        return Err(BundleError::Length);
    }
    if verify_container(&e.image.encoded, e.image.profile)? != e.image.rgb {
        return Err(BundleError::Profile);
    }

    let mut out = Vec::with_capacity(11 * PIXELS + e.image.encoded.len() + bundle.capture_state.len() + 4096);
    out.extend_from_slice(MAGIC);
    out.push(e.image.profile.code());
    blob(&mut out, &e.image.encoded);
    for (i, frame) in e.frames.iter().enumerate() {
        if i != 1 {
            blob(&mut out, &frame.rgb);
        }
        length(&mut out, frame.depth_mm.len());
        for depth in &frame.depth_mm {
            out.extend_from_slice(&depth.to_be_bytes());
        }
        blob(&mut out, &frame.conf);
    }
    for camera in &e.camera {
        for x in [camera.fx, camera.fy, camera.cx, camera.cy].into_iter().chain(camera.r).chain(camera.t) {
            out.extend_from_slice(&(x as i32).to_be_bytes());
        }
        out.extend_from_slice(&camera.ts_us.to_be_bytes());
        out.push(camera.track);
    }
    for g in e.imu.g0 {
        out.extend_from_slice(&g.to_be_bytes());
    }
    out.extend_from_slice(&e.imu.a_peak_mg.to_be_bytes());
    for window in &e.imu.windows {
        length(&mut out, window.len());
        for sample in window {
            out.extend_from_slice(&sample.dt_us.to_be_bytes());
            for w in sample.w {
                out.extend_from_slice(&w.to_be_bytes());
            }
        }
    }

    let bt = &e.bluetooth;
    out.extend_from_slice(&[bt.version, bt.flags]);
    for x in [bt.scan_duration_ms, bt.n_adv, bt.n_unique].into_iter().chain(bt.rssi_histogram) {
        out.extend_from_slice(&x.to_be_bytes());
    }
    out.extend_from_slice(&bt.rssi_mean_q8.to_be_bytes());
    out.extend_from_slice(&bt.rssi_var_q8.to_be_bytes());
    out.push(bt.persistence_pct);

    out.push(e.noise.version);
    length(&mut out, e.noise.stats.len());
    for stat in &e.noise.stats {
        out.extend_from_slice(&stat.to_be_bytes());
    }
    out.push(e.meta.version);
    out.extend_from_slice(&e.meta.app_build.to_be_bytes());
    out.extend_from_slice(&e.meta.device_model_code.to_be_bytes());
    for offset in e.meta.frame_offsets_ms {
        out.extend_from_slice(&offset.to_be_bytes());
    }
    out.push(e.meta.ycbcr_matrix_code);
    out.extend_from_slice(&e.meta.quality_flags.to_be_bytes());

    for fr in bundle.salts.as_array() {
        out.extend_from_slice(&fr_to_be(&fr));
    }
    blob(&mut out, &bundle.capture_state);
    Ok(out)
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], BundleError> {
        let (bytes, rest) = self.0.split_at_checked(n).ok_or(BundleError::Length)?;
        self.0 = rest;
        Ok(bytes)
    }

    fn word<const N: usize>(&mut self) -> Result<[u8; N], BundleError> {
        let (bytes, rest) = self.0.split_first_chunk::<N>().ok_or(BundleError::Length)?;
        self.0 = rest;
        Ok(*bytes)
    }

    fn u8(&mut self) -> Result<u8, BundleError> { Ok(self.word::<1>()?[0]) }
    fn u32(&mut self) -> Result<u32, BundleError> { Ok(u32::from_be_bytes(self.word()?)) }
    fn i32(&mut self) -> Result<i32, BundleError> { Ok(i32::from_be_bytes(self.word()?)) }

    fn len(&mut self, min: usize, max: usize) -> Result<usize, BundleError> {
        let n = self.u32()? as usize;
        if !(min..=max).contains(&n) {
            return Err(BundleError::Length);
        }
        Ok(n)
    }

    fn blob(&mut self, min: usize, max: usize) -> Result<&'a [u8], BundleError> {
        let n = self.len(min, max)?;
        self.take(n)
    }

    fn frame(&mut self, include_rgb: bool) -> Result<FrameData, BundleError> {
        let rgb = if include_rgb { self.blob(3 * PIXELS, 3 * PIXELS)?.to_vec() } else { Vec::new() };
        let depth_count = self.len(PIXELS, PIXELS)?;
        let depth_mm = self.take(depth_count * 2)?.chunks_exact(2)
            .map(|b| u16::from_be_bytes([b[0], b[1]])).collect();
        let conf = self.blob(PIXELS, PIXELS)?.to_vec();
        Ok(FrameData { rgb, depth_mm, conf })
    }

    fn camera(&mut self) -> Result<CameraFrame, BundleError> {
        let (fx, fy, cx, cy) = (self.i32()?, self.i32()?, self.i32()?, self.i32()?);
        let mut r = [0; 9];
        let mut t = [0; 3];
        for v in r.iter_mut().chain(&mut t) { *v = i64::from(self.i32()?); }
        let ts_us = u64::from_be_bytes(self.word()?);
        Ok(CameraFrame { fx: i64::from(fx), fy: i64::from(fy), cx: i64::from(cx), cy: i64::from(cy), r, t, ts_us, track: self.u8()? })
    }

    fn imu_window(&mut self) -> Result<Vec<ImuSample>, BundleError> {
        let count = self.len(IMU_MAX, IMU_MAX)?;
        (0..count).map(|_| Ok(ImuSample { dt_us: self.u32()?, w: [self.i32()?, self.i32()?, self.i32()?] })).collect()
    }

    fn bluetooth(&mut self) -> Result<BluetoothAggregate, BundleError> {
        let mut bt = BluetoothAggregate {
            version: self.u8()?, flags: self.u8()?, scan_duration_ms: self.u32()?, n_adv: self.u32()?, n_unique: self.u32()?,
            ..BluetoothAggregate::default()
        };
        for bin in &mut bt.rssi_histogram { *bin = self.u32()?; }
        bt.rssi_mean_q8 = self.i32()?;
        bt.rssi_var_q8 = self.u32()?;
        bt.persistence_pct = self.u8()?;
        Ok(bt)
    }
}

pub fn decode(bytes: &[u8]) -> Result<EvidenceBundle, BundleError> {
    let mut r = Reader(bytes.strip_prefix(MAGIC).ok_or(BundleError::Prefix)?);
    let profile = match r.u8()? {
        0 => ImageProfile::PngV1,
        1 => ImageProfile::JpegV1,
        _ => return Err(BundleError::Profile),
    };
    let encoded = r.blob(1, MAX_IMAGE_BYTES)?;
    let mut frames = [r.frame(true)?, r.frame(false)?, r.frame(true)?];
    let camera = [r.camera()?, r.camera()?, r.camera()?];
    let imu = ImuEvidence {
        g0: [r.i32()?, r.i32()?, r.i32()?],
        a_peak_mg: r.u32()?,
        windows: [r.imu_window()?, r.imu_window()?],
    };
    let bluetooth = r.bluetooth()?;
    let noise_version = r.u8()?;
    let noise_count = r.len(0, 16)?;
    let noise = NoiseAggregate {
        version: noise_version,
        stats: r.take(noise_count * 2)?.chunks_exact(2).map(|b| i16::from_be_bytes([b[0], b[1]])).collect(),
    };
    let meta = Meta {
        version: r.u8()?,
        image_profile: profile,
        app_build: r.u32()?,
        device_model_code: r.u32()?,
        frame_offsets_ms: [r.i32()?, r.i32()?, r.i32()?],
        ycbcr_matrix_code: r.u8()?,
        quality_flags: r.u32()?,
    };
    let mut salt_array = [galaxy_crypto::Fr::from(0u32); 8];
    for salt in &mut salt_array {
        *salt = fr_from_be_canonical(r.take(32)?).map_err(|_| BundleError::Field)?;
    }
    let salts = Salts::from_array(salt_array);
    let capture_state = r.blob(0, MAX_CAPTURE_STATE_BYTES)?;
    if !r.0.is_empty() {
        return Err(BundleError::Length);
    }
    let rgb = verify_container(encoded, profile)?;
    frames[1].rgb = analysis_raster(&rgb)?;
    let image = CanonicalImage { profile, rgb, encoded: encoded.to_vec() };
    let evidence = Evidence { image, frames, camera, imu, bluetooth, noise, meta };
    evidence.validate()?;
    Ok(EvidenceBundle { evidence, salts, capture_state: capture_state.to_vec() })
}
