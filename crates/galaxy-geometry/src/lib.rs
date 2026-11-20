//! Reference physical-consistency engine for policy `galaxy-pc-v1m` with exact integer arithmetic and named statistics, plus the synthetic scene generator and calibration tools (spec §8).

mod lidar;
mod motion;
mod temporal;
mod view;

pub mod diagnostics;
pub mod fixtures;
pub mod synthetic;

use galaxy_protocol::policy::{Lidar, Policy};
use serde::{Deserialize, Serialize};

pub use view::{build_view, Block, GeometryView, Pair};

const WIDTH: usize = 256;
const HEIGHT: usize = 192;
const BLOCKS: usize = 64;
const CANDIDATES: usize = 32;
const PAIRS: usize = 64;
const IMU_MAX: usize = 64;
const Q30: i128 = 1 << 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ViewError {
    #[error("invalid evidence frame or IMU buffer size")]
    EvidenceShape,
    #[error("invalid geometry view version, layout or input range")]
    ViewDomain,
    #[error("temporal block coordinates do not match the computed opening")]
    PairOpening,
    #[error("invalid policy")]
    Policy,
}

/// Named §8.8 statistics; field names in JSON are exactly those in the spec.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Statistics {
    #[serde(rename = "V_all")]
    pub v_all: u16,
    #[serde(rename = "V_c")]
    pub v_c: u16,
    pub n_hyp_ok_g: u16,
    pub n_hyp_ok_c: u16,
    pub max_all: u16,
    pub max_c: u16,
    pub j_star: u16,
    pub tiles_cov: u16,
    pub tiles_nonplanar: u16,
    pub tiles_nonplanar_c: u16,
    pub n_strong: u16,
    pub tiles_strong: u16,
    #[serde(rename = "N_disc")]
    pub n_disc: u16,
    #[serde(rename = "N_match")]
    pub n_match: u16,
    #[serde(rename = "N_usable")]
    pub n_usable: u16,
    #[serde(rename = "N_occ")]
    pub n_occ: u16,
    #[serde(rename = "N_dok")]
    pub n_dok: u16,
    #[serde(rename = "N_inf")]
    pub n_inf: u16,
    #[serde(rename = "N_pok")]
    pub n_pok: u16,
}

/// `pass` here covers only the five policy predicates; it says nothing about
/// commitments, challenge authenticity, or validity of a proof or capture.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyReport {
    #[serde(flatten)]
    pub stats: Statistics,
    pub quality_ok: bool,
    pub geometry_ok: bool,
    pub rgb_depth_ok: bool,
    pub temporal_ok: bool,
    pub motion_ok: bool,
    pub pass: bool,
}

/// Evaluate the five policy predicates; malformed inputs fail closed. Use
/// [`try_evaluate`] to distinguish malformed inputs from policy rejection.
pub fn evaluate(view: &GeometryView, policy: &Policy) -> PolicyReport {
    try_evaluate(view, policy).unwrap_or_default()
}

pub fn try_evaluate(view: &GeometryView, policy: &Policy) -> Result<PolicyReport, ViewError> {
    view.validate()?;
    evaluate_in_domain(view, policy)
}

fn evaluate_in_domain(view: &GeometryView, policy: &Policy) -> Result<PolicyReport, ViewError> {
    policy.validate().map_err(|_| ViewError::Policy)?;
    let relative = relative_poses(view);
    let mut stats = Statistics::default();
    let quality_ok = motion::quality(view, &policy.quality);
    let geometry_ok = lidar::planarity(view, policy, &mut stats);
    let rgb_depth_ok = lidar::correspondence(view, policy, &mut stats);
    let temporal_ok = temporal::evaluate(view, &relative, policy, &mut stats)?;
    let motion_ok = motion::evaluate(view, &relative, &policy.motion);
    Ok(PolicyReport {
        stats, quality_ok, geometry_ok, rgb_depth_ok, temporal_ok, motion_ok,
        pass: quality_ok && geometry_ok && rgb_depth_ok && temporal_ok && motion_ok,
    })
}

fn valid(depth: u16, confidence: u8, p: &Lidar) -> bool {
    (p.d_min_mm..=p.d_max_mm).contains(&i64::from(depth)) && i64::from(confidence) >= p.conf_min
}

fn luminance(r: u8, g: u8, b: u8) -> i64 {
    77 * i64::from(r) + 150 * i64::from(g) + 29 * i64::from(b)
}

// Counts fit in u16; widening the policy threshold avoids i64 overflow even
// for a caller-provided policy. Both sides of every comparison are exact.
fn pct(count: u16) -> i128 {
    100 * i128::from(count)
}

fn scaled(scale: i64, count: u16) -> i128 {
    i128::from(scale) * i128::from(count)
}

#[derive(Clone, Copy)]
struct RelativePose {
    r: [i128; 9],
    t: [i128; 3],
}

fn relative_poses(v: &GeometryView) -> [RelativePose; 2] {
    [0, 2].map(|j| RelativePose {
        r: std::array::from_fn(|i| {
            let (a, b) = (i / 3, i % 3);
            (0..3).map(|k| i128::from(v.r[j][3 * k + a]) * i128::from(v.r[1][3 * k + b]))
                .sum::<i128>().div_euclid(Q30)
        }),
        t: std::array::from_fn(|a| {
            (0..3).map(|k| i128::from(v.r[j][3 * k + a]) * i128::from(v.t[1][k] - v.t[j][k]))
                .sum::<i128>().div_euclid(Q30)
        }),
    })
}
