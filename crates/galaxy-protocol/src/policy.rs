//! Mobile physical-consistency policy (§8.7). Policy identity is over the canonical JSON, not the source formatting.
use crate::{canonical_json, sha256, Hash32};
use serde::{Deserialize, Serialize};
use std::{io, path::Path};

#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("reading policy: {0}")]
    Io(#[from] io::Error),
    #[error("decoding policy: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Canonical(#[from] canonical_json::Error),
    #[error("invalid policy field or group: {0}")]
    Invalid(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub name: String,
    pub version: u8,
    pub profile: String,
    pub structural: Structural,
    pub lidar: Lidar,
    pub rgb_depth: RgbDepth,
    pub temporal: Temporal,
    pub quality: Quality,
    pub motion: Motion,
    pub windows: Windows,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Structural {
    pub p: u64,
    pub coefficient_bound: u64,
    pub slots: u8,
    pub central_slots: u8,
    pub blocks_per_slot: u8,
    pub global_hypotheses: u8,
    pub central_hypotheses: u8,
    pub temporal_quadrants: [u8; 2],
    pub imu_max: u8,
}

impl Structural {
    pub const V1M: Self = Self {
        p: 1 << 32,
        coefficient_bound: 1 << 36,
        slots: 16,
        central_slots: 8,
        blocks_per_slot: 4,
        global_hypotheses: 4,
        central_hypotheses: 4,
        temporal_quadrants: [0, 3],
        imu_max: 64,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lidar {
    pub d_min_mm: i64,
    pub d_max_mm: i64,
    pub conf_min: i64,
    pub area_min: i64,
    pub eps_log2: i64,
    pub sep_log2: i64,
    pub v_all_min: i64,
    pub v_c_min: i64,
    pub tile_valid_min: i64,
    pub tiles_cov_min: i64,
    pub plane_global_max_pct: i64,
    pub plane_central_max_pct: i64,
    pub tile_out_min: i64,
    pub nonplanar_tiles_min: i64,
    pub nonplanar_tiles_c_min: i64,
    pub strong_min_pct: i64,
    pub strong_tiles_min: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RgbDepth {
    pub disc_pct: i64,
    pub grad_min_y: i64,
    pub disc_min: i64,
    pub edge_match_pct: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Temporal {
    pub occ_pm: i64,
    pub dcons_pm: i64,
    pub g_inf_y: i64,
    pub photo_tol_y: i64,
    pub t_usable_min: i64,
    pub occ_max_pct: i64,
    pub dok_min_pct: i64,
    pub inf_min: i64,
    pub pok_min_pct: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quality {
    pub dt_min_us: i64,
    pub dt_max_us: i64,
    pub fx_min_q16: i64,
    pub fx_max_q16: i64,
    pub cx_min_q16: i64,
    pub cx_max_q16: i64,
    pub cy_min_q16: i64,
    pub cy_max_q16: i64,
    pub ortho_tol_q60: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Motion {
    pub baseline_min_um: i64,
    pub rot_max_q30: i64,
    pub imu_min_samples: i64,
    pub imu_dt_max_us: i64,
    pub imu_cover_tol_us: i64,
    pub gyro_tol_urad: i64,
    pub m_cd: [i64; 9],
    pub grav_norm_tol_q40: i64,
    pub grav_cos_milli: i64,
    pub acc_max_mg: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Windows {
    pub w_cap: u32,
    pub w_proof: u32,
    pub sel_offset: u32,
}

impl Policy {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, PolicyError> {
        let policy: Self = serde_json::from_reader(io::BufReader::new(std::fs::File::open(path)?))?;
        policy.canonical_json()?;
        Ok(policy)
    }

    pub fn validate(&self) -> Result<(), PolicyError> {
        if self.version != 1 || self.profile != "galaxy-pc-v1m" || self.name.is_empty() || !self.name.is_ascii() {
            return Err(PolicyError::Invalid("name/version/profile"));
        }
        if self.structural != Structural::V1M {
            return Err(PolicyError::Invalid("structural"));
        }
        let l = &self.lidar;
        if !bounds(&[(l.d_min_mm, 1, l.d_max_mm), (l.d_max_mm, 1, 8191), (l.conf_min, 0, 3),
            (l.area_min, 1, 65_535), (l.eps_log2, 0, l.sep_log2), (l.sep_log2, 0, 36),
            (l.v_all_min, 0, 256), (l.v_c_min, 0, 128), (l.tile_valid_min, 0, 16),
            (l.tiles_cov_min, 0, 16), (l.plane_global_max_pct, 0, 100),
            (l.plane_central_max_pct, 0, 100), (l.tile_out_min, 0, 16),
            (l.nonplanar_tiles_min, 0, 16), (l.nonplanar_tiles_c_min, 0, 8),
            (l.strong_min_pct, 0, 100), (l.strong_tiles_min, 0, 16)]) {
            return Err(PolicyError::Invalid("lidar"));
        }
        let r = &self.rgb_depth;
        if !bounds(&[(r.disc_pct, 0, 200), (r.grad_min_y, 0, 65280), (r.disc_min, 0, 512), (r.edge_match_pct, 0, 100)]) {
            return Err(PolicyError::Invalid("rgb_depth"));
        }
        let t = &self.temporal;
        if !bounds(&[(t.occ_pm, 0, 1000), (t.dcons_pm, 0, 1000), (t.g_inf_y, 0, 65280),
            (t.photo_tol_y, 0, 65280), (t.t_usable_min, 0, 64), (t.occ_max_pct, 0, 100),
            (t.dok_min_pct, 0, 100), (t.inf_min, 0, 64), (t.pok_min_pct, 0, 100)]) {
            return Err(PolicyError::Invalid("temporal"));
        }
        let q = &self.quality;
        if !bounds(&[(q.dt_min_us, 1, q.dt_max_us), (q.dt_max_us, 1, (1 << 48) - 1),
            (q.fx_min_q16, 1, q.fx_max_q16), (q.fx_max_q16, 1, (1 << 26) - 1),
            (q.cx_min_q16, 0, q.cx_max_q16), (q.cx_max_q16, 0, (1 << 26) - 1),
            (q.cy_min_q16, 0, q.cy_max_q16), (q.cy_max_q16, 0, (1 << 26) - 1),
            (q.ortho_tol_q60, 0, (1 << 53) - 1)]) {
            return Err(PolicyError::Invalid("quality"));
        }
        let m = &self.motion;
        if !bounds(&[(m.baseline_min_um, 0, i64::from(u32::MAX)), (m.rot_max_q30, 0, i64::from(u32::MAX)),
            (m.imu_min_samples, 0, 64), (m.imu_dt_max_us, 1, i64::from(u32::MAX)),
            (m.imu_cover_tol_us, 0, i64::from(u32::MAX)), (m.gyro_tol_urad, 0, i64::from(u32::MAX)),
            (m.grav_norm_tol_q40, 0, (1 << 53) - 1), (m.grav_cos_milli, -1000, 1000),
            (m.acc_max_mg, 0, i64::from(u32::MAX))]) || !signed_permutation(&m.m_cd) {
            return Err(PolicyError::Invalid("motion"));
        }
        if self.windows.w_cap == 0 || self.windows.w_proof == 0 || self.windows.sel_offset == 0 {
            return Err(PolicyError::Invalid("windows"));
        }
        Ok(())
    }

    pub fn canonical_json(&self) -> Result<Vec<u8>, PolicyError> {
        self.validate()?;
        Ok(canonical_json::to_vec(self)?)
    }

    pub fn id(&self) -> Result<Hash32, PolicyError> {
        Ok(sha256(&[b"GALAXY/POLICY/v1\0", &self.canonical_json()?]))
    }
}

fn bounds(fields: &[(i64, i64, i64)]) -> bool {
    fields.iter().all(|&(x, min, max)| (min..=max).contains(&x))
}

fn signed_permutation(matrix: &[i64; 9]) -> bool {
    matrix.iter().all(|&x| (-1..=1).contains(&x))
        && matrix.chunks_exact(3).all(|r| r.iter().filter(|&&x| x != 0).count() == 1)
        && (0..3).all(|col| matrix[col..].iter().step_by(3).filter(|&&x| x != 0).count() == 1)
}
