use crate::{ImageError, ANALYSIS_HEIGHT, ANALYSIS_WIDTH, RGB_LEN, SOURCE_HEIGHT, SOURCE_WIDTH, WIDTH};

const SOURCE_STRIDE: usize = SOURCE_WIDTH * 3;
const PLANE_LEN: usize = SOURCE_WIDTH * SOURCE_HEIGHT;

fn check_planes(y: &[u8], cbcr: &[u8]) -> Result<(), ImageError> {
    if y.len() != PLANE_LEN || cbcr.len() != PLANE_LEN / 2 {
        return Err(ImageError::Unsupported("420f plane dimensions"));
    }
    Ok(())
}

fn convert_row(y: &[u8], cbcr: &[u8], rgb: &mut [u8]) {
    for ((ys, cs), output) in y.chunks_exact(2).zip(cbcr.chunks_exact(2)).zip(rgb.chunks_exact_mut(6)) {
        let cb = i32::from(cs[0]) - 128;
        let cr = i32::from(cs[1]) - 128;
        let r = (91881 * cr + 32768) >> 16;
        let g = -((22554 * cb + 46802 * cr + 32768) >> 16);
        let b = (116130 * cb + 32768) >> 16;
        for (&luma, pixel) in ys.iter().zip(output.chunks_exact_mut(3)) {
            let luma = i32::from(luma);
            pixel.copy_from_slice(&[r, g, b].map(|offset| (luma + offset).clamp(0, 255) as u8));
        }
    }
}

/// Convert tightly packed 1920×1440 420f Y and interleaved CbCr planes to RGB8.
pub fn ycbcr420_full_to_rgb(y: &[u8], cbcr: &[u8]) -> Result<Vec<u8>, ImageError> {
    check_planes(y, cbcr)?;
    let mut rgb = vec![0; PLANE_LEN * 3];
    for (row, output) in rgb.chunks_exact_mut(SOURCE_STRIDE).enumerate() {
        let i = row * SOURCE_WIDTH;
        let j = (row / 2) * SOURCE_WIDTH;
        convert_row(&y[i..i + SOURCE_WIDTH], &cbcr[j..j + SOURCE_WIDTH], output);
    }
    Ok(rgb)
}

const fn coverage(position: usize) -> (usize, [u32; 3]) {
    let start = 15 * position;
    let first = start / 8;
    let first_weight = 8 - start % 8;
    let remaining = 15 - first_weight;
    let second_weight = if remaining > 8 { 8 } else { remaining };
    (first, [first_weight as u32, second_weight as u32, (remaining - second_weight) as u32])
}

static X_COVERAGE: [(usize, [u32; 3]); WIDTH] = {
    let mut coverage_by_x = [(0, [0; 3]); WIDTH];
    let mut x = 0;
    while x < WIDTH {
        coverage_by_x[x] = coverage(x);
        x += 1;
    }
    coverage_by_x
};

fn box_row(rows: [&[u8]; 3], wy: [u32; 3], output: &mut [u8]) {
    for (pixel, &(x0, wx)) in output.chunks_exact_mut(3).zip(&X_COVERAGE) {
        let mut sums = [0; 3];
        for (row, wy) in rows.into_iter().zip(wy).filter(|&(_, w)| w != 0) {
            for (dx, wx) in wx.into_iter().enumerate().filter(|&(_, w)| w != 0) {
                let i = 3 * (x0 + dx);
                let w = wy * wx;
                for (sum, &value) in sums.iter_mut().zip(&row[i..i + 3]) {
                    *sum += w * u32::from(value);
                }
            }
        }
        pixel.copy_from_slice(&sums.map(|sum| ((sum + 112) / 225) as u8));
    }
}

/// Exact 8-unit box filter from sensor RGB8 to the 1024×768 canonical raster.
pub fn downscale_rgb(rgb: &[u8]) -> Result<Vec<u8>, ImageError> {
    if rgb.len() != PLANE_LEN * 3 {
        return Err(ImageError::Unsupported("source RGB dimensions"));
    }
    let mut canonical = vec![0; RGB_LEN];
    for (row, output) in canonical.chunks_exact_mut(WIDTH * 3).enumerate() {
        let (y0, weights) = coverage(row);
        let rows = std::array::from_fn(|i| {
            let offset = (y0 + i.min(1 + usize::from(weights[2] != 0))) * SOURCE_STRIDE;
            &rgb[offset..offset + SOURCE_STRIDE]
        });
        box_row(rows, weights, output);
    }
    Ok(canonical)
}

/// Fused conversion/box filter: each source pixel is converted once, with only three scratch rows.
pub fn ycbcr420_full_to_canonical(y: &[u8], cbcr: &[u8]) -> Result<Vec<u8>, ImageError> {
    check_planes(y, cbcr)?;
    let mut canonical = vec![0; RGB_LEN];
    let mut rows = [[0; SOURCE_STRIDE]; 3];
    let mut next_row = 0;
    for (row, output) in canonical.chunks_exact_mut(WIDTH * 3).enumerate() {
        let (y0, weights) = coverage(row);
        let last = y0 + 1 + usize::from(weights[2] != 0);
        while next_row <= last {
            let i = next_row * SOURCE_WIDTH;
            let j = (next_row / 2) * SOURCE_WIDTH;
            convert_row(&y[i..i + SOURCE_WIDTH], &cbcr[j..j + SOURCE_WIDTH], &mut rows[next_row % 3]);
            next_row += 1;
        }
        box_row(std::array::from_fn(|i| rows[(y0 + i) % 3].as_slice()), weights, output);
    }
    Ok(canonical)
}

/// The sampled centers are at x = 4u + 2 and y = 4v + 2, not box averages.
pub fn analysis_raster(rgb: &[u8]) -> Result<Vec<u8>, ImageError> {
    if rgb.len() != RGB_LEN {
        return Err(ImageError::Unsupported("canonical RGB dimensions"));
    }
    let mut analysis = Vec::with_capacity(ANALYSIS_WIDTH * ANALYSIS_HEIGHT * 3);
    for row in rgb.chunks_exact(WIDTH * 3).skip(2).step_by(4) {
        for pixel in row.chunks_exact(3).skip(2).step_by(4) {
            analysis.extend_from_slice(pixel);
        }
    }
    Ok(analysis)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clamp255(v: i32) -> u8 {
        v.clamp(0, 255) as u8
    }

    /// Spec §5 formula, one pixel at a time.
    fn reference_pixel(y: u8, cb: u8, cr: u8) -> [u8; 3] {
        let (y, cb, cr) = (i32::from(y), i32::from(cb) - 128, i32::from(cr) - 128);
        [
            clamp255(y + ((91881 * cr + 32768) >> 16)),
            clamp255(y - ((22554 * cb + 46802 * cr + 32768) >> 16)),
            clamp255(y + ((116130 * cb + 32768) >> 16)),
        ]
    }

    /// Spec §5 box weights, w(x) = max(0, min(8x+8, 15X+15) − max(8x, 15X)).
    fn weight(source: usize, output: usize) -> u64 {
        let (lo, hi) = ((8 * source).max(15 * output), (8 * source + 8).min(15 * output + 15));
        hi.saturating_sub(lo) as u64
    }

    fn pseudo_random(len: usize, seed: u32) -> Vec<u8> {
        let mut state = seed;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state >> 24) as u8
            })
            .collect()
    }

    #[test]
    fn conversion_matches_formula_for_every_chroma_pair() {
        // Each 2×2 luma block shares one chroma pair: cover all 65536 (Cb, Cr) pairs.
        let y = pseudo_random(PLANE_LEN, 7);
        let mut cbcr = vec![0; PLANE_LEN / 2];
        for (i, pair) in cbcr.chunks_exact_mut(2).enumerate() {
            pair.copy_from_slice(&[(i % 256) as u8, ((i / 256) % 256) as u8]);
        }
        let rgb = ycbcr420_full_to_rgb(&y, &cbcr).unwrap();
        for (i, pixel) in rgb.chunks_exact(3).enumerate() {
            let (x, row) = (i % SOURCE_WIDTH, i / SOURCE_WIDTH);
            let c = (row / 2) * SOURCE_WIDTH + (x / 2) * 2;
            assert_eq!(pixel, reference_pixel(y[i], cbcr[c], cbcr[c + 1]), "pixel {x},{row}");
        }
    }

    #[test]
    fn box_filter_matches_formula() {
        let source = pseudo_random(PLANE_LEN * 3, 11);
        let canonical = downscale_rgb(&source).unwrap();
        for out_y in 0..crate::HEIGHT {
            for out_x in 0..WIDTH {
                for channel in 0..3 {
                    let mut sum = 0u64;
                    for sy in (15 * out_y) / 8..=(15 * out_y + 14) / 8 {
                        for sx in (15 * out_x) / 8..=(15 * out_x + 14) / 8 {
                            let v = u64::from(source[(sy * SOURCE_WIDTH + sx) * 3 + channel]);
                            sum += weight(sx, out_x) * weight(sy, out_y) * v;
                        }
                    }
                    assert_eq!(u64::from(canonical[(out_y * WIDTH + out_x) * 3 + channel]), (sum + 112) / 225);
                }
            }
        }
        let weights: u64 = (0..SOURCE_WIDTH).map(|x| weight(x, 5)).sum();
        assert_eq!(weights, 15);
    }

    #[test]
    fn fused_pipeline_equals_two_step_pipeline() {
        let y = pseudo_random(PLANE_LEN, 3);
        let cbcr = pseudo_random(PLANE_LEN / 2, 5);
        let two_step = downscale_rgb(&ycbcr420_full_to_rgb(&y, &cbcr).unwrap()).unwrap();
        assert_eq!(ycbcr420_full_to_canonical(&y, &cbcr).unwrap(), two_step);
    }

    #[test]
    fn analysis_raster_samples_block_centers() {
        let rgb = pseudo_random(RGB_LEN, 9);
        let analysis = analysis_raster(&rgb).unwrap();
        assert_eq!(analysis.len(), ANALYSIS_WIDTH * ANALYSIS_HEIGHT * 3);
        for v in 0..ANALYSIS_HEIGHT {
            for u in 0..ANALYSIS_WIDTH {
                let a = (v * ANALYSIS_WIDTH + u) * 3;
                let r = ((4 * v + 2) * WIDTH + 4 * u + 2) * 3;
                assert_eq!(analysis[a..a + 3], rgb[r..r + 3]);
            }
        }
    }

    #[test]
    fn wrong_sizes_are_rejected() {
        assert!(ycbcr420_full_to_rgb(&[0; 10], &[0; 5]).is_err());
        assert!(ycbcr420_full_to_canonical(&vec![0; PLANE_LEN], &[0; 5]).is_err());
        assert!(downscale_rgb(&[0; 30]).is_err());
        assert!(analysis_raster(&[0; 30]).is_err());
    }
}
