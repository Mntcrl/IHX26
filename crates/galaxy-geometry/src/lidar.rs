use galaxy_protocol::policy::Lidar;

use crate::{luminance, pct, scaled, valid, GeometryView, Policy, Statistics, BLOCKS};

const P: i128 = 1 << 32;
const COEFFICIENT_LIMIT: i128 = 1 << 36;
const POINTS: usize = BLOCKS * 4;
const CENTRAL_POINTS: usize = POINTS / 2;

#[derive(Clone, Copy)]
struct Point {
    u: i128,
    v: i128,
    d: i128,
    valid: bool,
}

/// Outcome of fitting one §8.2 hypothesis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fit {
    /// Equal indices, an invalid vertex, or |Δ| < area_min.
    PreDegenerate,
    /// Some coefficient has magnitude ≥ 2^36.
    CoefficientBound,
    Plane([i128; 3]),
}

impl Fit {
    fn plane(self) -> Option<[i128; 3]> {
        match self {
            Self::Plane(c) => Some(c),
            _ => None,
        }
    }
}

fn fit([p0, p1, p2]: [Point; 3], idx: [u16; 3], area_min: i64) -> Fit {
    if idx[0] == idx[1] || idx[0] == idx[2] || idx[1] == idx[2] || !p0.valid || !p1.valid || !p2.valid {
        return Fit::PreDegenerate;
    }
    let (u0, u1, u2) = (p0.u, p1.u, p2.u);
    let (v0, v1, v2) = (p0.v, p1.v, p2.v);
    let delta = u0 * (v1 - v2) - v0 * (u1 - u2) + (u1 * v2 - u2 * v1);
    if delta.abs() < i128::from(area_min) {
        return Fit::PreDegenerate;
    }
    let (w0, w1, w2) = (p1.d * p2.d, p0.d * p2.d, p0.d * p1.d);
    let det_a = w0 * (v1 - v2) - v0 * (w1 - w2) + (w1 * v2 - w2 * v1);
    let det_b = u0 * (w1 - w2) - w0 * (u1 - u2) + (u1 * w2 - u2 * w1);
    let det_c = u0 * (v1 * w2 - v2 * w1) - v0 * (u1 * w2 - u2 * w1) + w0 * (u1 * v2 - u2 * v1);
    let den = p0.d * w0 * delta.abs();
    // Positive by the validated d_min_mm and area_min. Largest determinant
    // uses two 8-bit coordinates and two <13-bit depths; P adds 32 bits.
    let coeff = [det_a, det_b, det_c].map(|det| (delta.signum() * det * P).div_euclid(den));
    if coeff.iter().all(|a| a.abs() < COEFFICIENT_LIMIT) { Fit::Plane(coeff) } else { Fit::CoefficientBound }
}

fn residual(p: Point, [a, b, c]: [i128; 3]) -> i128 {
    P - p.d * (a * p.u + b * p.v + c)
}

fn points(v: &GeometryView, p: &Lidar) -> [Point; POINTS] {
    std::array::from_fn(|i| {
        let (block, k) = (i / 4, i % 4);
        let (dx, dy) = (1 + k % 2, 1 + k / 2);
        let px = 4 * dy + dx;
        Point {
            u: i128::from(v.blk_u0[block]) + dx as i128,
            v: i128::from(v.blk_v0[block]) + dy as i128,
            d: i128::from(v.blk_d[block][px]),
            valid: valid(v.blk_d[block][px], v.blk_c[block][px], p),
        }
    })
}

/// Per-hypothesis fit outcome (j = 0..7), for fixture construction and diagnostics.
pub(crate) fn hypothesis_fits(v: &GeometryView, p: &Lidar) -> [Fit; 8] {
    let points = points(v, p);
    std::array::from_fn(|j| {
        let idx = v.hyp_idx[j];
        fit(idx.map(|i| points[usize::from(i)]), idx, p.area_min)
    })
}

pub(crate) fn planarity(v: &GeometryView, policy: &Policy, s: &mut Statistics) -> bool {
    let p = &policy.lidar;
    let points = points(v, p);
    let eps = 1i128 << p.eps_log2;
    let sep = 1i128 << p.sep_log2;
    let mut best = None;
    for (j, &idx) in v.hyp_idx.iter().enumerate() {
        let coeff = fit(idx.map(|i| points[usize::from(i)]), idx, p.area_min).plane();
        let mut support_all = 0;
        let mut support_c = 0;
        if let Some(coeff) = coeff {
            if j < 4 {
                s.n_hyp_ok_g += 1;
            } else {
                s.n_hyp_ok_c += 1;
            }
            let n = if j < 4 { POINTS } else { CENTRAL_POINTS };
            for (i, &point) in points[..n].iter().enumerate() {
                if point.valid && (-eps..eps).contains(&residual(point, coeff)) {
                    support_all += 1;
                    support_c += u16::from(i < CENTRAL_POINTS);
                }
            }
        }
        if j == 0 || (j < 4 && support_all > s.max_all) {
            s.max_all = support_all;
            s.j_star = j as u16;
            best = coeff;
        }
        s.max_c = s.max_c.max(support_c);
    }
    for (slot, chunk) in points.chunks_exact(16).enumerate() {
        let mut valid_s = 0;
        let mut out_s = 0;
        let mut strong_s = 0;
        for &point in chunk.iter().filter(|point| point.valid) {
            valid_s += 1;
            if let Some(coeff) = best {
                let r = residual(point, coeff);
                out_s += u16::from(!(-eps..eps).contains(&r));
                strong_s += u16::from(!(-sep..sep).contains(&r));
            } else {
                out_s += 1;
            }
        }
        s.v_all += valid_s;
        if slot < 8 {
            s.v_c += valid_s;
            s.tiles_nonplanar_c += u16::from(i64::from(out_s) >= p.tile_out_min);
        }
        s.tiles_cov += u16::from(i64::from(valid_s) >= p.tile_valid_min);
        s.tiles_nonplanar += u16::from(i64::from(out_s) >= p.tile_out_min);
        s.n_strong += strong_s;
        s.tiles_strong += u16::from(strong_s > 0);
    }
    i64::from(s.v_all) >= p.v_all_min
        && i64::from(s.v_c) >= p.v_c_min
        && i64::from(s.tiles_cov) >= p.tiles_cov_min
        && s.n_hyp_ok_g > 0 && s.n_hyp_ok_c > 0
        && pct(s.max_all) <= scaled(p.plane_global_max_pct, s.v_all)
        && pct(s.max_c) <= scaled(p.plane_central_max_pct, s.v_c)
        && i64::from(s.tiles_nonplanar) >= p.nonplanar_tiles_min
        && i64::from(s.tiles_nonplanar_c) >= p.nonplanar_tiles_c_min
        && pct(s.n_strong) >= scaled(p.strong_min_pct, s.v_all)
        && i64::from(s.tiles_strong) >= p.strong_tiles_min
}

pub(crate) fn correspondence(v: &GeometryView, policy: &Policy, s: &mut Statistics) -> bool {
    let p = &policy.rgb_depth;
    for b in 0..BLOCKS {
        let d = &v.blk_d[b];
        let c = &v.blk_c[b];
        let y: [i64; 16] = std::array::from_fn(|i| luminance(v.blk_r[b][i], v.blk_g[b][i], v.blk_b[b][i]));
        let disc = |i: usize, step: usize| {
            let (a, b) = (i - step, i + step);
            valid(d[a], c[a], &policy.lidar) && valid(d[b], c[b], &policy.lidar)
                && 200 * i64::from(d[a].abs_diff(d[b])) >= p.disc_pct * (i64::from(d[a]) + i64::from(d[b]))
        };
        let grad = |i: usize, step: usize| (y[i - step] - y[i + step]).abs() >= p.grad_min_y;
        for i in [5, 6, 9, 10] {
            if disc(i, 1) {
                s.n_disc += 1;
                s.n_match += u16::from([i - 4, i, i + 4].into_iter().any(|k| grad(k, 1)));
            }
            if disc(i, 4) {
                s.n_disc += 1;
                s.n_match += u16::from([i - 1, i, i + 1].into_iter().any(|k| grad(k, 4)));
            }
        }
    }
    i64::from(s.n_disc) >= p.disc_min && pct(s.n_match) >= scaled(p.edge_match_pct, s.n_disc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(u: i128, v: i128, d: i128) -> Point {
        Point { u, v, d, valid: true }
    }

    #[test]
    fn plane_floor_and_orientation() {
        let points = [point(1, 1, 1000), point(41, 1, 1300), point(1, 41, 1000)];
        let Fit::Plane(c) = fit(points, [0, 1, 2], 64) else { panic!("plane expected") };
        assert_eq!(c[0], (-300 * P).div_euclid(40 * 1000 * 1300));
        assert_eq!(c[1], 0);
        assert_eq!(fit([points[0], points[2], points[1]], [0, 2, 1], 64), Fit::Plane(c));
    }

    #[test]
    fn plane_degeneracy_and_coefficient_overflow() {
        let points = [point(193, 185, 2500), point(237, 181, 2500), point(245, 182, 2500)];
        assert!(matches!(fit(points, [0, 1, 2], 64), Fit::Plane(_))); // |delta| = 76
        assert_eq!(fit(points, [0, 1, 2], 77), Fit::PreDegenerate);
        assert_eq!(fit(points, [0, 0, 2], 64), Fit::PreDegenerate);
        assert_eq!(fit([Point { valid: false, ..points[0] }, points[1], points[2]], [0, 1, 2], 64), Fit::PreDegenerate);
        // A validated alternate policy may allow 1 mm, unlike checked-in v1m.
        // The intercept exceeds 2^36 when extrapolating these shallow-angle vertices to the origin.
        assert_eq!(fit([points[0], points[1], point(245, 182, 1)], [0, 1, 2], 64), Fit::CoefficientBound);
    }
}
