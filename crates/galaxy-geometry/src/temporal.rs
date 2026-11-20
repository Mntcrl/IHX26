use ethnum::I256;

use crate::{luminance, pct, scaled, valid, GeometryView, Pair, Policy, RelativePose, Statistics, ViewError, HEIGHT, PAIRS, Q30, WIDTH};

const Q16: i128 = 1 << 16;

pub(crate) struct Projection {
    position: Option<[I256; 2]>,
    qz: i128,
    m: i128,
}

impl Projection {
    fn pixel(&self) -> Option<[usize; 2]> {
        let [u, v] = self.position?;
        let inside = |x: I256, limit: usize| x >= I256::ZERO && x < I256::from(limit as u64);
        (inside(u, WIDTH) && inside(v, HEIGHT)).then(|| [u.as_usize(), v.as_usize()])
    }

    pub(crate) fn block_xy(&self) -> (u8, u8) {
        self.pixel().map_or((0, 0), |[x, y]| ((x >> 2) as u8, (y >> 2) as u8))
    }

    pub(crate) fn metadata(&self, p: usize) -> Pair {
        let (slot, q, frame) = pair_indices(p);
        let [uj, vj] = self.position.map_or([None, None], |xy| xy.map(|x| Some(x.to_string())));
        Pair { slot: slot as u8, q: q as u8, frame: frame as u8, inb: self.pixel().is_some(), uj, vj }
    }
}

fn pair_indices(p: usize) -> (usize, usize, usize) {
    (p / 4, 3 * ((p / 2) % 2), 2 * (p % 2))
}

fn candidate(v: &GeometryView, p: usize) -> (usize, usize, i128, i128) {
    let (slot, q, _) = pair_indices(p);
    let b = 4 * slot + q;
    let ix = usize::from(v.tmp_ix[p / 2]);
    let iy = usize::from(v.tmp_iy[p / 2]);
    (b, 4 * iy + ix, i128::from(v.blk_u0[b]) + ix as i128, i128::from(v.blk_v0[b]) + iy as i128)
}

pub(crate) fn project(v: &GeometryView, relative: &[RelativePose; 2], p: usize) -> Projection {
    let (b, k, u0, v0) = candidate(v, p);
    let (_, _, j) = pair_indices(p);
    let rel = relative[j / 2];
    let d0 = i128::from(v.blk_d[b][k]);
    let (fx, fy) = (i128::from(v.fx[1]), i128::from(v.fy[1]));
    let f = fx * fy;
    let a0 = u0 * Q16 - i128::from(v.cx[1]);
    let b0 = v0 * Q16 - i128::from(v.cy[1]);
    let point = [a0 * d0 * fy, b0 * d0 * fx, d0 * f];
    // Validated §20 operands give |P| < 2^68, |R_j1| < 2^34 and
    // |T_j1| < 2^35, hence all Q components fit below 2^118 in i128.
    let q: [i128; 3] = std::array::from_fn(|a| {
        1000 * (0..3).map(|k| rel.r[3 * a + k] * point[k]).sum::<i128>() + Q30 * f * rel.t[a]
    });
    let position = (q[2] > 0).then(|| {
        let z = I256::from(q[2]);
        let den = z * I256::from(Q16);
        std::array::from_fn(|axis| {
            let [focal, center] = [[v.fx[j], v.cx[j]], [v.fy[j], v.cy[j]]][axis];
            let num = I256::from(focal) * I256::from(q[axis]) + I256::from(i128::from(center) + Q16 / 2) * z;
            num.div_euclid(den)
        })
    });
    Projection { position, qz: q[2], m: 1000 * Q30 * f }
}

fn depth_flags(qz: i128, m: i128, depth: u16, occ_pm: i64, dcons_pm: i64) -> (bool, bool) {
    // Scale in 256 bits as Q_z can approach the i128 limit after multiplying by 1000.
    let expected = I256::from(qz);
    let observed = I256::from(depth) * I256::from(m);
    let occluded = observed * 1000 < expected * I256::from(1000 - occ_pm);
    let depth_ok = !occluded && (observed - expected).abs() * 1000 <= observed * I256::from(dcons_pm);
    (occluded, depth_ok)
}

pub(crate) fn evaluate(v: &GeometryView, relative: &[RelativePose; 2], policy: &Policy, s: &mut Statistics) -> Result<bool, ViewError> {
    let p = &policy.temporal;
    for i in 0..PAIRS {
        let proj = project(v, relative, i);
        if proj.block_xy() != (v.pr_bx[i], v.pr_by[i]) {
            return Err(ViewError::PairOpening);
        }
        let Some([u, y]) = proj.pixel() else { continue };
        let (b, k, _, _) = candidate(v, i);
        let kj = 4 * (y % 4) + u % 4;
        let dj = v.pr_d[i][kj];
        if !valid(v.blk_d[b][k], v.blk_c[b][k], &policy.lidar) || !valid(dj, v.pr_c[i][kj], &policy.lidar) {
            continue;
        }
        s.n_usable += 1;
        let (occluded, depth_ok) = depth_flags(proj.qz, proj.m, dj, p.occ_pm, p.dcons_pm);
        s.n_dok += u16::from(depth_ok);
        if occluded {
            s.n_occ += 1;
            continue;
        }
        let y0 = |k| luminance(v.blk_r[b][k], v.blk_g[b][k], v.blk_b[b][k]);
        let center = y0(k);
        if [k - 1, k + 1, k - 4, k + 4].into_iter().any(|n| (center - y0(n)).abs() >= p.g_inf_y) {
            s.n_inf += 1;
            s.n_pok += u16::from((0..16).any(|n| {
                (center - luminance(v.pr_r[i][n], v.pr_g[i][n], v.pr_b[i][n])).abs() <= p.photo_tol_y
            }));
        }
    }
    Ok(i64::from(s.n_usable) >= p.t_usable_min
        && pct(s.n_occ) <= scaled(p.occ_max_pct, s.n_usable)
        && pct(s.n_dok) >= scaled(p.dok_min_pct, s.n_usable - s.n_occ)
        && i64::from(s.n_inf) >= p.inf_min
        && pct(s.n_pok) >= scaled(p.pok_min_pct, s.n_inf))
}

#[cfg(test)]
mod tests {
    use super::depth_flags;

    #[test]
    fn depth_tolerance_is_inclusive_both_directions() {
        assert_eq!(depth_flags(1030, 1, 1000, 50, 30), (false, true));
        assert_eq!(depth_flags(970, 1, 1000, 50, 30), (false, true));
        assert_eq!(depth_flags(1031, 1, 1000, 50, 30), (false, false));
        assert_eq!(depth_flags(969, 1, 1000, 50, 30), (false, false));
    }

    #[test]
    fn occlusion_uses_strict_inequality() {
        assert_eq!(depth_flags(1000, 1, 950, 50, 30), (false, false));
        assert_eq!(depth_flags(1000, 1, 949, 50, 30), (true, false));
    }
}
