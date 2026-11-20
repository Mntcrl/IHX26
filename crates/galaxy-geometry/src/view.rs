use galaxy_crypto::sampling::Challenge;
use galaxy_evidence::{CameraFrame, Evidence, FrameData, ImuEvidence};
use serde::{Deserialize, Serialize};

use crate::{relative_poses, temporal, Policy, PolicyReport, ViewError, BLOCKS, CANDIDATES, IMU_MAX, PAIRS, WIDTH, HEIGHT};

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Block {
    pub slot: u8,
    pub q: u8,
    /// Block x-coordinate within its tile, not within the whole raster.
    pub bx: u8,
    pub by: u8,
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pair {
    pub slot: u8,
    pub q: u8,
    pub frame: u8,
    pub inb: bool,
    /// With positive Z these are signed (possibly far outside the raster).
    #[serde(deserialize_with = "Option::<String>::deserialize")]
    pub uj: Option<String>,
    #[serde(deserialize_with = "Option::<String>::deserialize")]
    pub vj: Option<String>,
}

/// `galaxy-geometry-view/v1` (§20). Contains private witness values; do not log.
/// `expected` is a diagnostic annotation, never an input to evaluation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeometryView {
    #[serde(deserialize_with = "version_one")]
    pub version: u8,
    pub tiles: [u8; 16],
    #[serde(with = "array")]
    pub blocks: [Block; BLOCKS],
    #[serde(with = "array")]
    pub pairs: [Pair; PAIRS],
    #[serde(with = "array")]
    pub blk_r: [[u8; 16]; BLOCKS],
    #[serde(with = "array")]
    pub blk_g: [[u8; 16]; BLOCKS],
    #[serde(with = "array")]
    pub blk_b: [[u8; 16]; BLOCKS],
    #[serde(with = "array")]
    pub blk_d: [[u16; 16]; BLOCKS],
    #[serde(with = "array")]
    pub blk_c: [[u8; 16]; BLOCKS],
    #[serde(with = "array")]
    pub blk_u0: [u16; BLOCKS],
    #[serde(with = "array")]
    pub blk_v0: [u16; BLOCKS],
    pub tmp_ix: [u8; CANDIDATES],
    pub tmp_iy: [u8; CANDIDATES],
    pub hyp_idx: [[u16; 3]; 8],
    #[serde(with = "array")]
    pub pr_r: [[u8; 16]; PAIRS],
    #[serde(with = "array")]
    pub pr_g: [[u8; 16]; PAIRS],
    #[serde(with = "array")]
    pub pr_b: [[u8; 16]; PAIRS],
    #[serde(with = "array")]
    pub pr_d: [[u16; 16]; PAIRS],
    #[serde(with = "array")]
    pub pr_c: [[u8; 16]; PAIRS],
    #[serde(with = "array")]
    pub pr_bx: [u8; PAIRS],
    #[serde(with = "array")]
    pub pr_by: [u8; PAIRS],
    pub fx: [i64; 3],
    pub fy: [i64; 3],
    pub cx: [i64; 3],
    pub cy: [i64; 3],
    #[serde(rename = "R")]
    pub r: [[i64; 9]; 3],
    #[serde(rename = "T")]
    pub t: [[i64; 3]; 3],
    pub ts: [u64; 3],
    pub track: [u8; 3],
    #[serde(with = "imu_arrays")]
    pub imu_dt: [[u32; IMU_MAX]; 2],
    #[serde(with = "imu_arrays")]
    pub imu_w: [[[i32; 3]; IMU_MAX]; 2],
    pub g0: [i32; 3],
    pub a_peak: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<PolicyReport>,
}

impl GeometryView {
    fn new(camera: &[CameraFrame; 3], imu: &ImuEvidence, c: &Challenge) -> Self {
        Self {
            version: 1, tiles: c.tiles,
            blocks: [Block::default(); BLOCKS], pairs: std::array::from_fn(|_| Pair::default()),
            blk_r: [[0; 16]; BLOCKS], blk_g: [[0; 16]; BLOCKS], blk_b: [[0; 16]; BLOCKS],
            blk_d: [[0; 16]; BLOCKS], blk_c: [[0; 16]; BLOCKS],
            blk_u0: [0; BLOCKS], blk_v0: [0; BLOCKS],
            tmp_ix: [0; CANDIDATES], tmp_iy: [0; CANDIDATES], hyp_idx: c.hyp,
            pr_r: [[0; 16]; PAIRS], pr_g: [[0; 16]; PAIRS], pr_b: [[0; 16]; PAIRS],
            pr_d: [[0; 16]; PAIRS], pr_c: [[0; 16]; PAIRS], pr_bx: [0; PAIRS], pr_by: [0; PAIRS],
            fx: camera.map(|f| f.fx), fy: camera.map(|f| f.fy),
            cx: camera.map(|f| f.cx), cy: camera.map(|f| f.cy),
            r: camera.map(|f| f.r), t: camera.map(|f| f.t),
            ts: camera.map(|f| f.ts_us), track: camera.map(|f| f.track),
            imu_dt: std::array::from_fn(|w| std::array::from_fn(|k| imu.windows[w][k].dt_us)),
            imu_w: std::array::from_fn(|w| std::array::from_fn(|k| imu.windows[w][k].w)),
            g0: imu.g0, a_peak: imu.a_peak_mg, expected: None,
        }
    }

    /// Annotate this diagnostic view with its computed §8.8 report.
    pub fn with_expected(mut self, policy: &Policy) -> Self {
        self.expected = Some(crate::evaluate(&self, policy));
        self
    }

    pub(crate) fn validate(&self) -> Result<(), ViewError> {
        let main_bounds = self.version == 1
            && self.fx.iter().chain(&self.fy).chain(&self.cx).chain(&self.cy).all(|x| (0..1 << 26).contains(x))
            && self.r.iter().flatten().chain(self.t.iter().flatten()).all(|&x| i32::try_from(x).is_ok())
            && self.ts.iter().all(|&x| x < 1 << 48) && self.track.iter().all(|&x| x < 4)
            && self.blk_c.iter().chain(&self.pr_c).flatten().all(|&x| x < 4)
            && self.tmp_ix.iter().chain(&self.tmp_iy).all(|x| (1..=2).contains(x))
            && self.hyp_idx.iter().enumerate().all(|(j, row)| row.iter().all(|&i| i < if j < 4 { 256 } else { 128 }));
        if !main_bounds {
            return Err(ViewError::ViewDomain);
        }
        for (slot, &tile) in self.tiles.iter().enumerate() {
            let (tx, ty) = (tile % 8, tile / 8);
            let central = (2..=5).contains(&tx) && (1..=4).contains(&ty);
            if ty >= 6 || central != (slot < 8)
                || (slot % 8 != 0 && self.tiles[slot - 1] >= tile)
            {
                return Err(ViewError::ViewDomain);
            }
            for q in 0..4u8 {
                let b = 4 * slot + usize::from(q);
                let block = self.blocks[b];
                if usize::from(block.slot) != slot || block.q != q || !in_quadrant(q, block.bx, block.by)
                    || self.blk_u0[b] != 32 * u16::from(tx) + 4 * u16::from(block.bx)
                    || self.blk_v0[b] != 32 * u16::from(ty) + 4 * u16::from(block.by)
                {
                    return Err(ViewError::ViewDomain);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn open_pairs(&mut self, frames: &[FrameData; 3]) {
        let relative = relative_poses(self);
        for p in 0..PAIRS {
            let projection = temporal::project(self, &relative, p);
            let (bx, by) = projection.block_xy();
            let pair = projection.metadata(p);
            let frame = usize::from(pair.frame);
            self.pairs[p] = pair;
            self.pr_bx[p] = bx;
            self.pr_by[p] = by;
            open_block(&frames[frame], 4 * usize::from(bx), 4 * usize::from(by),
                [&mut self.pr_r[p], &mut self.pr_g[p], &mut self.pr_b[p]], &mut self.pr_d[p], &mut self.pr_c[p]);
        }
    }
}


fn in_quadrant(q: u8, bx: u8, by: u8) -> bool {
    bx / 4 == q % 2 && by / 4 == q / 2
}

fn open_block(frame: &FrameData, u0: usize, v0: usize, [r, g, b]: [&mut [u8; 16]; 3], d: &mut [u16; 16], c: &mut [u8; 16]) {
    for k in 0..16 {
        let source = WIDTH * (v0 + k / 4) + u0 + k % 4;
        r[k] = frame.rgb[3 * source];
        g[k] = frame.rgb[3 * source + 1];
        b[k] = frame.rgb[3 * source + 2];
        d[k] = frame.depth_mm[source];
        c[k] = frame.conf[source];
    }
}

/// Construct a policy-independent view, checking all source buffer shapes and
/// circuit input bounds before exposing it to the evaluator/witness builder.
pub fn build_view(e: &Evidence, c: &Challenge) -> Result<GeometryView, ViewError> {
    view_from_parts(&e.frames, &e.camera, &e.imu, c)
}

/// [`build_view`] over the only Evidence parts the view depends on.
pub(crate) fn view_from_parts(frames: &[FrameData; 3], camera: &[CameraFrame; 3], imu: &ImuEvidence, c: &Challenge) -> Result<GeometryView, ViewError> {
    const PIXELS: usize = WIDTH * HEIGHT;
    for frame in frames {
        if frame.rgb.len() != PIXELS * 3 || frame.conf.len() != PIXELS || frame.depth_mm.len() != PIXELS {
            return Err(ViewError::EvidenceShape);
        }
        if frame.conf.iter().any(|&x| x >= 4) {
            return Err(ViewError::ViewDomain);
        }
    }
    if !imu.windows.iter().all(|window| window.len() == IMU_MAX) {
        return Err(ViewError::EvidenceShape);
    }
    let mut v = GeometryView::new(camera, imu, c);
    for (slot, (&tile, picks)) in c.tiles.iter().zip(&c.picks).enumerate() {
        if tile >= 48 {
            return Err(ViewError::ViewDomain);
        }
        for q in 0..4u8 {
            let b = 4 * slot + usize::from(q);
            let (bx, by) = picks.blocks[usize::from(q)];
            if !in_quadrant(q, bx, by) {
                return Err(ViewError::ViewDomain);
            }
            v.blocks[b] = Block { slot: slot as u8, q, bx, by };
            let (u0, v0) = (32 * u16::from(tile % 8) + 4 * u16::from(bx), 32 * u16::from(tile / 8) + 4 * u16::from(by));
            v.blk_u0[b] = u0;
            v.blk_v0[b] = v0;
            open_block(&frames[1], usize::from(u0), usize::from(v0),
                [&mut v.blk_r[b], &mut v.blk_g[b], &mut v.blk_b[b]], &mut v.blk_d[b], &mut v.blk_c[b]);
        }
        for (qi, q) in [0, 3].into_iter().enumerate() {
            let (ix, iy) = picks.temporal[q];
            v.tmp_ix[2 * slot + qi] = ix;
            v.tmp_iy[2 * slot + qi] = iy;
        }
    }
    v.validate()?;
    v.open_pairs(frames);
    Ok(v)
}

fn version_one<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    match u8::deserialize(d)? {
        1 => Ok(1),
        _ => Err(serde::de::Error::custom("unsupported geometry view version")),
    }
}

// Serde only implements array traits up to length 32. Keep the fixed-size
// circuit inputs typed and serialize as tuples without intermediate buffers.
mod array {
    use serde::de::{Error, SeqAccess, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::marker::PhantomData;

    pub(super) fn serialize<T: Serialize, S: Serializer, const N: usize>(a: &[T; N], s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeTuple;
        let mut out = s.serialize_tuple(N)?;
        for item in a { out.serialize_element(item)?; }
        out.end()
    }

    pub(super) fn deserialize<'de, T: Deserialize<'de> + Default, D: Deserializer<'de>, const N: usize>(d: D) -> Result<[T; N], D::Error> {
        struct Array<T, const N: usize>(PhantomData<T>);
        impl<'de, T: Deserialize<'de> + Default, const N: usize> Visitor<'de> for Array<T, N> {
            type Value = [T; N];
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "a tuple of length {N}") }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
                let mut out = std::array::from_fn(|_| T::default());
                for (i, item) in out.iter_mut().enumerate() {
                    *item = a.next_element()?.ok_or_else(|| Error::invalid_length(i, &self))?;
                }
                Ok(out)
            }
        }
        d.deserialize_tuple(N, Array::<T, N>(PhantomData))
    }
}

mod imu_arrays {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use super::IMU_MAX;

    pub(super) fn serialize<T: Serialize, S: Serializer>(a: &[[T; IMU_MAX]; 2], s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(transparent)]
        struct Window<'a, T: Serialize>(#[serde(serialize_with = "super::array::serialize")] &'a [T; IMU_MAX]);
        [Window(&a[0]), Window(&a[1])].serialize(s)
    }

    pub(super) fn deserialize<'de, T: Deserialize<'de> + Default, D: Deserializer<'de>>(d: D) -> Result<[[T; IMU_MAX]; 2], D::Error> {
        #[derive(Deserialize)]
        #[serde(transparent, bound(deserialize = "T: Deserialize<'de> + Default"))]
        struct Window<T>(#[serde(deserialize_with = "super::array::deserialize")] [T; IMU_MAX]);
        Ok(<[Window<T>; 2]>::deserialize(d)?.map(|window| window.0))
    }
}
