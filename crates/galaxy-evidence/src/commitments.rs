use galaxy_crypto::{
    commit as crypto,
    field::{fr_from_be_canonical, fr_from_i128},
    tree::{FrameTree, TreeKind},
    Fr,
};

use crate::{Evidence, Salts, ValidationError, BLUETOOTH_ELEMENTS, CAMERA_ELEMENTS, IMU_ELEMENTS, META_ELEMENTS, NOISE_ELEMENTS};

pub struct Commitments {
    pub rgb_tree: FrameTree,
    pub depth_tree: FrameTree,
    pub minus_tree: FrameTree,
    pub plus_tree: FrameTree,
    pub components: [Fr; 16],
    pub rgb_root: Fr,
    pub evidence_root: Fr,
}

impl Salts {
    pub fn random() -> Result<Self, getrandom::Error> {
        let mut entropy = [[0u8; 31]; 8];
        getrandom::fill(entropy.as_flattened_mut())?;
        // Every value fits in 248 bits, strictly below the BN254 scalar modulus.
        let salts = entropy.map(|bytes| {
            let mut be = [0; 32];
            be[1..].copy_from_slice(&bytes);
            fr_from_be_canonical(&be).expect("31-byte salt is canonical")
        });
        Ok(Self::from_array(salts))
    }

    pub(crate) fn from_array(values: [Fr; 8]) -> Self {
        let [depth0, frame_minus, frame_plus, camera, imu, bluetooth, noise, meta] = values;
        Self { depth0, frame_minus, frame_plus, camera, imu, bluetooth, noise, meta }
    }

    pub(crate) fn as_array(&self) -> [Fr; 8] {
        [self.depth0, self.frame_minus, self.frame_plus, self.camera, self.imu, self.bluetooth, self.noise, self.meta]
    }
}

/// Scalar component preimages, in slots 4 through 8 (CAMERA, IMU, BLUETOOTH, NOISE, META).
/// The input must satisfy [`Evidence::validate`].
pub fn component_elements(evidence: &Evidence) -> [Vec<Fr>; 5] {
    let mut camera = Vec::with_capacity(CAMERA_ELEMENTS);
    for frame in &evidence.camera {
        camera.extend([frame.fx, frame.fy, frame.cx, frame.cy].into_iter().map(signed));
        camera.extend(frame.r.into_iter().map(signed));
        camera.extend(frame.t.into_iter().map(signed));
        camera.extend([Fr::from(frame.ts_us), Fr::from(frame.track)]);
    }

    let mut imu = Vec::with_capacity(IMU_ELEMENTS);
    imu.extend(evidence.imu.g0.map(|g| signed(i64::from(g))));
    imu.extend([Fr::from(evidence.imu.a_peak_mg), Fr::from(0u32)]);
    for sample in evidence.imu.windows.iter().flatten() {
        let [x, y, z] = sample.w.map(|w| (w as u32) ^ (1 << 31));
        let value = (u128::from(sample.dt_us) << 96) | (u128::from(x) << 64)
            | (u128::from(y) << 32) | u128::from(z);
        imu.push(Fr::from(value));
    }

    let bt = &evidence.bluetooth;
    let mut bluetooth = Vec::with_capacity(BLUETOOTH_ELEMENTS);
    bluetooth.extend([Fr::from(bt.version), Fr::from(bt.flags), Fr::from(bt.scan_duration_ms), Fr::from(bt.n_adv), Fr::from(bt.n_unique)]);
    bluetooth.extend(bt.rssi_histogram.map(Fr::from));
    bluetooth.extend([signed(i64::from(bt.rssi_mean_q8)), Fr::from(bt.rssi_var_q8), Fr::from(bt.persistence_pct)]);

    let mut noise = Vec::with_capacity(NOISE_ELEMENTS);
    noise.extend([Fr::from(evidence.noise.version), Fr::from(evidence.noise.stats.len() as u64)]);
    noise.extend(evidence.noise.stats.iter().map(|&x| signed(i64::from(x))));
    noise.resize(NOISE_ELEMENTS, Fr::from(0u32));

    let m = &evidence.meta;
    let mut meta = Vec::with_capacity(META_ELEMENTS);
    meta.extend([Fr::from(m.version), Fr::from(m.image_profile.code()), Fr::from(m.app_build), Fr::from(m.device_model_code)]);
    meta.extend(m.frame_offsets_ms.map(|x| signed(i64::from(x))));
    meta.extend([Fr::from(m.ycbcr_matrix_code), Fr::from(m.quality_flags)]);

    [camera, imu, bluetooth, noise, meta]
}

fn signed(value: i64) -> Fr {
    fr_from_i128(i128::from(value))
}

/// Compute the commitment of checked evidence. Panics for malformed hand-built evidence;
/// use [`try_commit`] if evidence did not come from successful ingest or bundle decode.
pub fn commit(evidence: &Evidence, salts: &Salts) -> Commitments {
    try_commit(evidence, salts).expect("commit requires validated evidence")
}

pub fn try_commit(evidence: &Evidence, salts: &Salts) -> Result<Commitments, ValidationError> {
    evidence.validate()?;
    let [minus, center, plus] = &evidence.frames;
    let rgb_tree = FrameTree::build(TreeKind::Rgb, 1, &center.rgb, &[], &[]).map_err(|_| ValidationError::Frame)?;
    let depth_tree = FrameTree::build(TreeKind::Depth, 1, &[], &center.depth_mm, &center.conf).map_err(|_| ValidationError::Frame)?;
    let minus_tree = FrameTree::build(TreeKind::Frame, 0, &minus.rgb, &minus.depth_mm, &minus.conf).map_err(|_| ValidationError::Frame)?;
    let plus_tree = FrameTree::build(TreeKind::Frame, 2, &plus.rgb, &plus.depth_mm, &plus.conf).map_err(|_| ValidationError::Frame)?;
    let rgb_root = rgb_tree.root();
    let mut components = [Fr::from(0u32); 16];
    components[..4].copy_from_slice(&[
        rgb_root,
        crypto::tree_component(1, salts.depth0, depth_tree.root()),
        crypto::tree_component(2, salts.frame_minus, minus_tree.root()),
        crypto::tree_component(3, salts.frame_plus, plus_tree.root()),
    ]);
    let scalar_salts = [salts.camera, salts.imu, salts.bluetooth, salts.noise, salts.meta];
    for (i, (elements, salt)) in component_elements(evidence).into_iter().zip(scalar_salts).enumerate() {
        components[4 + i] = crypto::scalar_component((4 + i) as u8, salt, &elements);
    }
    for (slot, component) in components.iter_mut().enumerate().skip(9) {
        *component = crypto::pad_component(slot as u8);
    }
    let evidence_root = crypto::evidence_root(&components);
    Ok(Commitments { rgb_tree, depth_tree, minus_tree, plus_tree, components, rgb_root, evidence_root })
}
