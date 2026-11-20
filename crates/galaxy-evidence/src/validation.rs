use galaxy_image::{ANALYSIS_HEIGHT, ANALYSIS_WIDTH, MAX_IMAGE_BYTES, RGB_LEN, WIDTH};
use thiserror::Error;

use crate::{BluetoothAggregate, Evidence, NoiseAggregate, IMU_MAX};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum ValidationError {
    #[error("invalid image evidence")]
    Image,
    #[error("invalid frame evidence")]
    Frame,
    #[error("invalid camera parameters")]
    Camera,
    #[error("invalid IMU window")]
    Imu,
    #[error("invalid Bluetooth aggregate")]
    Bluetooth,
    #[error("invalid noise aggregate")]
    Noise,
    #[error("invalid capture metadata")]
    Meta,
}

impl BluetoothAggregate {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        let scanned = self.flags & 4 != 0;
        if self.version > 1
            || self.flags & !7 != 0
            || (self.version == 0 && *self != Self::default())
            || (scanned && self.flags != 7)
            || self.n_unique > self.n_adv
            || self.persistence_pct > 100
            || self.rssi_histogram.iter().map(|&v| u64::from(v)).sum::<u64>() != u64::from(self.n_adv)
            || !(-100 * 256..=0).contains(&self.rssi_mean_q8)
            || self.rssi_var_q8 > 2500 * 256
            || (!scanned && (self.scan_duration_ms != 0 || self.n_adv != 0))
            || (self.n_adv == 0 && (self.rssi_mean_q8 != 0 || self.rssi_var_q8 != 0 || self.persistence_pct != 0))
        {
            return Err(ValidationError::Bluetooth);
        }
        Ok(())
    }
}

impl NoiseAggregate {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        if self.version > 1 || self.stats.len() > 16 || (self.version == 0 && !self.stats.is_empty()) {
            return Err(ValidationError::Noise);
        }
        Ok(())
    }
}

pub(crate) fn offset_ms(ts: u64, center: u64) -> Result<i32, ValidationError> {
    // Both timestamps have already been bounded below 2^48.
    let delta = ts as i64 - center as i64;
    let rounded = delta.signum() * ((delta.abs() + 500) / 1000);
    i32::try_from(rounded).map_err(|_| ValidationError::Meta)
}

impl Evidence {
    pub fn validate(&self) -> Result<(), ValidationError> {
        const PIXELS: usize = ANALYSIS_WIDTH * ANALYSIS_HEIGHT;
        if self.image.rgb.len() != RGB_LEN || self.image.encoded.is_empty() || self.image.encoded.len() > MAX_IMAGE_BYTES {
            return Err(ValidationError::Image);
        }
        for frame in &self.frames {
            if frame.rgb.len() != 3 * PIXELS || frame.depth_mm.len() != PIXELS
                || frame.conf.len() != PIXELS || frame.conf.iter().any(|&conf| conf > 2)
            {
                return Err(ValidationError::Frame);
            }
        }
        let sampled_center = self.image.rgb.chunks_exact(WIDTH * 3).skip(2).step_by(4)
            .flat_map(|row| row.chunks_exact(3).skip(2).step_by(4));
        if !self.frames[1].rgb.chunks_exact(3).eq(sampled_center) {
            return Err(ValidationError::Image);
        }
        for camera in &self.camera {
            if [camera.fx, camera.fy, camera.cx, camera.cy].into_iter().any(|v| !(0..(1 << 26)).contains(&v))
                || camera.r.iter().chain(&camera.t).any(|&v| i32::try_from(v).is_err())
                || camera.ts_us >= (1 << 48) || camera.track > 2
            {
                return Err(ValidationError::Camera);
            }
        }
        if !self.camera.windows(2).all(|c| c[0].ts_us < c[1].ts_us) {
            return Err(ValidationError::Camera);
        }
        for (i, window) in self.imu.windows.iter().enumerate() {
            if window.len() != IMU_MAX {
                return Err(ValidationError::Imu);
            }
            let mut padding = false;
            let mut dt = 0u64;
            for sample in window {
                if sample.dt_us == 0 {
                    if sample.w != [0; 3] {
                        return Err(ValidationError::Imu);
                    }
                    padding = true;
                } else if padding {
                    return Err(ValidationError::Imu);
                }
                dt += u64::from(sample.dt_us);
            }
            if dt > self.camera[i + 1].ts_us - self.camera[i].ts_us {
                return Err(ValidationError::Imu);
            }
        }
        self.bluetooth.validate()?;
        self.noise.validate()?;
        if self.meta.version != 1 || self.meta.image_profile != self.image.profile || self.meta.ycbcr_matrix_code > 4 {
            return Err(ValidationError::Meta);
        }
        for (camera, &offset) in self.camera.iter().zip(&self.meta.frame_offsets_ms) {
            if offset_ms(camera.ts_us, self.camera[1].ts_us)? != offset {
                return Err(ValidationError::Meta);
            }
        }
        Ok(())
    }
}
