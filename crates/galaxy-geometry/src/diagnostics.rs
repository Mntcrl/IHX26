//! Aggregate diagnostics only; never return per-capture sensor values.

use serde::Serialize;

use crate::{motion, relative_poses, GeometryView, Policy, Q30};

#[derive(Serialize)]
pub struct ImuFrameFit {
    pub captures_used: usize,
    pub captures_excluded: usize,
    pub m_cd: [i64; 9],
    pub rms_urad: f64,
    pub max_axis_residual_urad: f64,
    pub runner_up_rms_urad: f64,
    pub policy_rms_urad: f64,
    /// Every signed permutation with its RMS residual, best first.
    pub candidates: Vec<Candidate>,
}

#[derive(Serialize)]
pub struct Candidate {
    pub m_cd: [i64; 9],
    pub rms_urad: f64,
}

#[derive(Clone, Default)]
struct Residuals {
    sq: f64,
    peak: f64,
}

/// Compare every signed permutation with §8.5's signed pose/gyro equations.
/// Compare the runner-up gap, residual and coverage before accepting a fit:
/// an under-excited data set cannot identify device axes.
pub fn fit_imu_frame(views: &[GeometryView], policy: &Policy) -> Option<ImuFrameFit> {
    policy.validate().ok()?;
    const PERMUTATIONS: [[usize; 3]; 6] = [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]];
    let matrices: [[i64; 9]; 48] = std::array::from_fn(|i| {
        let mut m = [0; 9];
        for (row, &axis) in PERMUTATIONS[i / 8].iter().enumerate() {
            m[3 * row + axis] = if i & (1 << row) == 0 { 1 } else { -1 };
        }
        m
    });
    let mut totals: [Residuals; 48] = std::array::from_fn(|_| Residuals::default());
    let mut used = 0;
    // Tracking state and intrinsics do not enter the pose/gyro comparison, so
    // only the view domain and the §8.5 IMU window rules gate a capture.
    for view in views.iter().filter(|v| v.validate().is_ok() && (0..2).all(|w| motion::imu_window(v, &policy.motion, w))) {
        used += 1;
        for (w, pose) in relative_poses(view).iter().enumerate() {
            let skew = motion::vee(&pose.r);
            let theta = motion::integrate_gyro(view, w);
            let sign = if w == 0 { 1 } else { -1 };
            for (matrix, total) in matrices.iter().zip(&mut totals) {
                for (a, b) in skew.into_iter().zip(motion::transform(matrix, theta)) {
                    let err = (a * 1_000_000 - sign * (1 << 11) * b).abs() as f64 / (2 * Q30) as f64;
                    total.sq += err * err;
                    total.peak = total.peak.max(err);
                }
            }
        }
    }
    if used == 0 { return None; }
    let mut ranked: [usize; 48] = std::array::from_fn(|i| i);
    ranked.sort_unstable_by(|&a, &b| totals[a].sq.total_cmp(&totals[b].sq).then(a.cmp(&b)));
    let best = ranked[0];
    let rms = |i: usize| libm::sqrt(totals[i].sq / (6.0 * used as f64));
    let current = matrices.iter().position(|m| *m == policy.motion.m_cd)?;
    Some(ImuFrameFit {
        captures_used: used, captures_excluded: views.len() - used,
        m_cd: matrices[best], rms_urad: rms(best), max_axis_residual_urad: totals[best].peak,
        runner_up_rms_urad: rms(ranked[1]), policy_rms_urad: rms(current),
        candidates: ranked.iter().map(|&i| Candidate { m_cd: matrices[i], rms_urad: rms(i) }).collect(),
    })
}
