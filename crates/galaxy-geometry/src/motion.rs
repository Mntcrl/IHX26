use galaxy_protocol::policy::{Motion, Quality};

use crate::{GeometryView, RelativePose, IMU_MAX, Q30};

fn dot(a: [i128; 3], b: [i128; 3]) -> i128 {
    a.into_iter().zip(b).map(|(x, y)| x * y).sum()
}

fn cross(a: [i128; 3], b: [i128; 3]) -> [i128; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn row(r: &[i64; 9], i: usize) -> [i128; 3] {
    [i128::from(r[3 * i]), i128::from(r[3 * i + 1]), i128::from(r[3 * i + 2])]
}

fn time_delta(v: &GeometryView, w: usize) -> i128 {
    i128::from(v.ts[w + 1]) - i128::from(v.ts[w])
}

fn in_range(x: i128, min: i64, max: i64) -> bool {
    (i128::from(min)..=i128::from(max)).contains(&x)
}

pub(crate) fn quality(v: &GeometryView, p: &Quality) -> bool {
    // Rows contain i32 values. Products, dot products and determinants fit
    // in i128 (determinant is a sum of six signed triple products).
    (0..3).all(|f| {
        let r = &v.r[f];
        let [a, b, c] = [row(r, 0), row(r, 1), row(r, 2)];
        let rows = [a, b, c];
        v.track[f] == 2
            && (p.fx_min_q16..=p.fx_max_q16).contains(&v.fx[f])
            && (p.fx_min_q16..=p.fx_max_q16).contains(&v.fy[f])
            && (p.cx_min_q16..=p.cx_max_q16).contains(&v.cx[f])
            && (p.cy_min_q16..=p.cy_max_q16).contains(&v.cy[f])
            && rows.iter().enumerate().all(|(i, &a)| {
                rows.iter().enumerate().skip(i).all(|(j, &b)| {
                    (dot(a, b) - if i == j { 1 << 60 } else { 0 }).abs() <= i128::from(p.ortho_tol_q60)
                })
            })
            && dot(cross(a, b), c) > 0
    }) && (0..2).all(|w| in_range(time_delta(v, w), p.dt_min_us, p.dt_max_us))
}

pub(crate) fn vee(r: &[i128; 9]) -> [i128; 3] {
    [r[7] - r[5], r[2] - r[6], r[3] - r[1]]
}

pub(crate) fn imu_window(v: &GeometryView, p: &Motion, w: usize) -> bool {
    let mut padded = false;
    let mut count = 0;
    let mut sum = 0;
    for k in 0..IMU_MAX {
        let dt = i64::from(v.imu_dt[w][k]);
        if dt == 0 {
            if v.imu_w[w][k] != [0; 3] {
                return false;
            }
            padded = true;
        } else {
            if padded || dt > p.imu_dt_max_us {
                return false;
            }
            count += 1;
            sum += i128::from(dt);
        }
    }
    count >= p.imu_min_samples && (sum - time_delta(v, w)).abs() <= i128::from(p.imu_cover_tol_us)
}

pub(crate) fn integrate_gyro(v: &GeometryView, w: usize) -> [i128; 3] {
    let mut theta = [0; 3];
    for (&dt, omega) in v.imu_dt[w].iter().zip(&v.imu_w[w]) {
        for (d, &rate) in theta.iter_mut().zip(omega) {
            *d += i128::from(dt) * i128::from(rate);
        }
    }
    theta
}

pub(crate) fn transform(matrix: &[i64; 9], v: [i128; 3]) -> [i128; 3] {
    std::array::from_fn(|i| dot(row(matrix, i), v))
}

pub(crate) fn evaluate(v: &GeometryView, relative: &[RelativePose; 2], p: &Motion) -> bool {
    let base = i128::from(p.baseline_min_um);
    let gyro_limit = 2 * i128::from(p.gyro_tol_urad) * Q30;
    for (w, pose) in relative.iter().enumerate() {
        let skew = vee(&pose.r);
        if dot(pose.t, pose.t) < base * base
            || skew.iter().any(|a| a.abs() > i128::from(p.rot_max_q30))
            || !imu_window(v, p, w)
        {
            return false;
        }
        let theta = transform(&p.m_cd, integrate_gyro(v, w));
        let sign = if w == 0 { 1 } else { -1 };
        if skew.into_iter().zip(theta).any(|(a, b)| (a * 1_000_000 - sign * (1 << 11) * b).abs() > gyro_limit) {
            return false;
        }
    }
    let gc = transform(&p.m_cd, v.g0.map(i128::from));
    let gp = row(&v.r[1], 1).map(|x| -x);
    (dot(gc, gc) - (1 << 40)).abs() <= i128::from(p.grav_norm_tol_q40)
        && 1000 * dot(gc, gp) >= i128::from(p.grav_cos_milli) * (1 << 50)
        && i64::from(v.a_peak) <= p.acc_max_mg
}
