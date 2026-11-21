//! Circuit input builder: turns evidence plus the ledger challenge into the
//! circom witness input JSON for `GalaxyPhysicalConsistencyProofV1`
//! (spec §9, schema in `circuits/INPUTS.md`).
//!
//! The produced JSON contains private evidence (pixels, depth, salts, sensor
//! values). Callers must keep it on the device and never log it.

use galaxy_crypto::{
    commit::{evidence_root, pad_component, scalar_component, tree_component},
    field::{fr_to_be, fr_to_dec},
    layout::{leaf_slot, tile_label, tile_xy, WIDTH},
    public::public_inputs_v1,
    sampling::{ctx, hypothesis_indices, select_tiles, slot_picks, Challenge},
    tree::{depth_leaf, frame_leaf, pack_depth, pack_rgb, rgb_leaf, FrameTree, TreeKind},
    Fr,
};
use galaxy_evidence::{component_elements, Commitments, Evidence, FrameData, Salts};
use galaxy_geometry::{build_view, GeometryView};
use galaxy_protocol::Hash32;
use serde_json::{json, Value};

/// Sampled tiles, blocks per tile, temporal pairs (spec §7, §8.4).
const TILES: usize = 16;
const PAIRS: usize = 64;
/// Frame-tree depth (arity 4) and the split between block and tile levels.
const LEVELS: usize = 6;
const BLOCK_LEVELS: usize = 3;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum WitnessError {
    #[error("evidence is outside the circuit input domain")]
    EvidenceDomain,
    #[error("salt is outside the 248-bit input domain")]
    SaltDomain,
    #[error("ledger challenge is not consistent with its seed and capture id")]
    Challenge,
    #[error("geometry view does not match the sampled opening layout")]
    View,
    #[error("commitments do not match the evidence and salts")]
    Commitment,
    #[error("could not open a sampled frame tree position")]
    Opening,
}

/// Public layout shared with the verifier (§9, `public_inputs_v1`). The
/// verifier reconstructs the seed and sampled tiles from the validated ledger,
/// never from witness JSON.
pub fn public_inputs(
    commitments: &Commitments,
    capture_id: &Hash32,
    challenge: &Challenge,
) -> [Fr; 22] {
    public_inputs_v1(
        &commitments.rgb_root,
        &commitments.evidence_root,
        capture_id,
        &challenge.seed,
        &challenge.tiles,
    )
}

/// Build the projection view with the reference engine, then the circuit input.
pub fn build_input_from_evidence(
    evidence: &Evidence,
    salts: &Salts,
    commitments: &Commitments,
    challenge: &Challenge,
    capture_id: &Hash32,
) -> Result<Value, WitnessError> {
    let view = build_view(evidence, challenge).map_err(|_| WitnessError::View)?;
    build_input(evidence, salts, commitments, challenge, capture_id, &view)
}

/// Build the canonical decimal-string circom input (every value in [0, r)).
///
/// The view only selects which temporal blocks to open (`pr_bx`, `pr_by`);
/// the circuit derives the sampling itself and constrains the projection.
/// Every opening is checked against its committed root here so that an
/// inconsistent caller fails before witness generation (fail closed).
pub fn build_input(
    evidence: &Evidence,
    salts: &Salts,
    commitments: &Commitments,
    challenge: &Challenge,
    capture_id: &Hash32,
    view: &GeometryView,
) -> Result<Value, WitnessError> {
    evidence
        .validate()
        .map_err(|_| WitnessError::EvidenceDomain)?;
    check_challenge(challenge, capture_id)?;
    if view.version != 1 || view.tiles != challenge.tiles {
        return Err(WitnessError::View);
    }
    let salt_fields = [
        salts.depth0,
        salts.frame_minus,
        salts.frame_plus,
        salts.camera,
        salts.imu,
        salts.bluetooth,
        salts.noise,
        salts.meta,
    ];
    // Circuit: Num2Bits(248) per salt.
    if salt_fields.iter().any(|f| fr_to_be(f)[0] != 0) {
        return Err(WitnessError::SaltDomain);
    }

    let elements = component_elements(evidence);
    let roots = [
        commitments.depth_tree.root(),
        commitments.minus_tree.root(),
        commitments.plus_tree.root(),
    ];
    check_commitments(commitments, &salt_fields, &roots, &elements)?;
    let [camera, imu, bluetooth, noise, meta] = elements;
    let publics = public_inputs(commitments, capture_id, challenge).map(|f| fr_to_dec(&f));

    let mut blk_rgb = Vec::with_capacity(TILES);
    let mut blk_depth = Vec::with_capacity(TILES);
    let mut blk_rgb_siblings = Vec::with_capacity(TILES);
    let mut blk_depth_siblings = Vec::with_capacity(TILES);
    let mut tile_rgb_siblings = Vec::with_capacity(TILES);
    let mut tile_depth_siblings = Vec::with_capacity(TILES);
    for (s, &tile) in challenge.tiles.iter().enumerate() {
        let (tx, ty) = tile_xy(tile);
        let mut rgb = Vec::with_capacity(4);
        let mut depth = Vec::with_capacity(4);
        let mut rgb_siblings = Vec::with_capacity(4);
        let mut depth_siblings = Vec::with_capacity(4);
        for (q, &(bx, by)) in challenge.picks[s].blocks.iter().enumerate() {
            let vb = &view.blocks[4 * s + q];
            if usize::from(vb.slot) != s || usize::from(vb.q) != q || (vb.bx, vb.by) != (bx, by) {
                return Err(WitnessError::View);
            }
            let (l, dl) = pack_block(&evidence.frames[1], 8 * tx + bx, 8 * ty + by);
            let slot = leaf_slot(tile, bx, by);
            let rp = open(
                &commitments.rgb_tree,
                TreeKind::Rgb,
                1,
                slot,
                rgb_leaf(1, slot, l),
            )?;
            let dp = open(
                &commitments.depth_tree,
                TreeKind::Depth,
                1,
                slot,
                depth_leaf(1, slot, dl),
            )?;
            rgb.push(decimal_pair(l));
            depth.push(decimal_pair(dl));
            rgb_siblings.push(rows(&rp[..BLOCK_LEVELS]));
            depth_siblings.push(rows(&dp[..BLOCK_LEVELS]));
            // The upper (tile-to-root) path is shared by the four blocks of a tile.
            if q == 0 {
                tile_rgb_siblings.push(rows(&rp[BLOCK_LEVELS..]));
                tile_depth_siblings.push(rows(&dp[BLOCK_LEVELS..]));
            }
        }
        blk_rgb.push(rgb);
        blk_depth.push(depth);
        blk_rgb_siblings.push(rgb_siblings);
        blk_depth_siblings.push(depth_siblings);
    }

    let mut pr_bx = Vec::with_capacity(PAIRS);
    let mut pr_by = Vec::with_capacity(PAIRS);
    let mut pr_rgb = Vec::with_capacity(PAIRS);
    let mut pr_depth = Vec::with_capacity(PAIRS);
    let mut pr_siblings = Vec::with_capacity(PAIRS);
    for p in 0..PAIRS {
        let (bx, by) = (view.pr_bx[p], view.pr_by[p]);
        // Pair p = 2c + ji uses frame 0 (ji = 0) or frame 2 (ji = 1).
        let frame = 2 * (p % 2) as u8;
        let pair = &view.pairs[p];
        if bx >= 64 || by >= 48 || pair.frame != frame || (!pair.inb && (bx, by) != (0, 0)) {
            return Err(WitnessError::View);
        }
        let tree = if frame == 0 {
            &commitments.minus_tree
        } else {
            &commitments.plus_tree
        };
        let (l, dl) = pack_block(&evidence.frames[usize::from(frame)], bx, by);
        let slot = leaf_slot(tile_label(bx / 8, by / 8), bx % 8, by % 8);
        let path = open(
            tree,
            TreeKind::Frame,
            frame,
            slot,
            frame_leaf(frame, slot, l, dl),
        )?;
        pr_bx.push(bx.to_string());
        pr_by.push(by.to_string());
        pr_rgb.push(decimal_pair(l));
        pr_depth.push(decimal_pair(dl));
        pr_siblings.push(rows(&path));
    }

    Ok(json!({
        "rgb_root": publics[0],
        "evidence_root": publics[1],
        "capture_id_hi": publics[2],
        "capture_id_lo": publics[3],
        "seed_hi": publics[4],
        "seed_lo": publics[5],
        "sampled_tiles": &publics[6..],
        "roots": decimals(&roots),
        "salts": decimals(&salt_fields),
        "camera": decimals(&camera),
        "imu": decimals(&imu),
        "bluetooth": decimals(&bluetooth),
        "noise": decimals(&noise),
        "meta": decimals(&meta),
        "blk_rgb": blk_rgb,
        "blk_depth": blk_depth,
        "blk_rgb_siblings": blk_rgb_siblings,
        "blk_depth_siblings": blk_depth_siblings,
        "tile_rgb_siblings": tile_rgb_siblings,
        "tile_depth_siblings": tile_depth_siblings,
        "pr_bx": pr_bx,
        "pr_by": pr_by,
        "pr_rgb": pr_rgb,
        "pr_depth": pr_depth,
        "pr_siblings": pr_siblings,
    }))
}

/// `Challenge` has public fields; recompute everything derived from the seed.
fn check_challenge(challenge: &Challenge, capture_id: &Hash32) -> Result<(), WitnessError> {
    let expected_ctx = ctx(capture_id, &challenge.seed);
    let consistent = challenge.ctx == expected_ctx
        && challenge.tiles == select_tiles(&challenge.seed)
        && challenge.hyp == hypothesis_indices(&expected_ctx)
        && (0..TILES)
            .all(|s| challenge.picks[s] == slot_picks(s, &expected_ctx, challenge.tiles[s]));
    if consistent {
        Ok(())
    } else {
        Err(WitnessError::Challenge)
    }
}

fn check_commitments(
    c: &Commitments,
    salts: &[Fr; 8],
    roots: &[Fr; 3],
    elements: &[Vec<Fr>; 5],
) -> Result<(), WitnessError> {
    let mut components = [Fr::default(); 16];
    components[0] = c.rgb_root;
    for (i, root) in roots.iter().enumerate() {
        components[i + 1] = tree_component(i as u8 + 1, salts[i], *root);
    }
    for (i, e) in elements.iter().enumerate() {
        components[i + 4] = scalar_component(i as u8 + 4, salts[i + 3], e);
    }
    for (i, component) in components.iter_mut().enumerate().skip(9) {
        *component = pad_component(i as u8);
    }
    if c.rgb_tree.root() != c.rgb_root
        || components != c.components
        || evidence_root(&components) != c.evidence_root
    {
        return Err(WitnessError::Commitment);
    }
    Ok(())
}

/// Leaf packing of the 4×4 block at block coordinates (bx, by) of the 64×48 grid.
fn pack_block(frame: &FrameData, bx: u8, by: u8) -> ((Fr, Fr), (Fr, Fr)) {
    let mut rgb = [0; 48];
    let mut depth = [0; 16];
    let mut conf = [0; 16];
    let first = usize::from(by) * 4 * WIDTH + usize::from(bx) * 4;
    for y in 0..4 {
        let start = first + y * WIDTH;
        rgb[12 * y..12 * y + 12].copy_from_slice(&frame.rgb[start * 3..start * 3 + 12]);
        depth[4 * y..4 * y + 4].copy_from_slice(&frame.depth_mm[start..start + 4]);
        conf[4 * y..4 * y + 4].copy_from_slice(&frame.conf[start..start + 4]);
    }
    (pack_rgb(&rgb), pack_depth(&depth, &conf))
}

fn open(
    tree: &FrameTree,
    kind: TreeKind,
    frame: u8,
    slot: u16,
    leaf: Fr,
) -> Result<[[Fr; 3]; LEVELS], WitnessError> {
    let opening = tree.opening(slot).map_err(|_| WitnessError::Opening)?;
    match opening.root_from_leaf(kind, frame, leaf) {
        Ok(root) if root == tree.root() => Ok(opening.siblings),
        _ => Err(WitnessError::Commitment),
    }
}

fn rows(levels: &[[Fr; 3]]) -> Vec<Vec<String>> {
    levels.iter().map(|row| decimals(row)).collect()
}

fn decimal_pair((a, b): (Fr, Fr)) -> [String; 2] {
    [fr_to_dec(&a), fr_to_dec(&b)]
}

fn decimals(values: &[Fr]) -> Vec<String> {
    values.iter().map(fr_to_dec).collect()
}
