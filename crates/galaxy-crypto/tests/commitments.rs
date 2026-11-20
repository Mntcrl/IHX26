use galaxy_crypto::{commit::*, field::fr_from_dec, layout::{leaf_slot, tile_xy, HEIGHT, WIDTH}, tree::*, Fr};
use serde::Deserialize;

#[derive(Deserialize)]
struct Vectors<T> { cases: Vec<T> }
#[derive(Deserialize)]
struct FrameCase { kind: u8, frame: u8, root: String, openings: Vec<OpeningCase> }
#[derive(Deserialize)]
struct OpeningCase { tile: u8, bx: u8, by: u8, slot: u16, packed: [String; 4], leaf: String, siblings: [[String; 3]; 6] }
#[derive(Deserialize)]
struct CommitCase { tree_roots: [String; 4], salts: [String; 8], elements: [Vec<String>; 5], components: [String; 16], root: String }

fn fr(s: String) -> Fr { fr_from_dec(&s).unwrap() }
fn pixel(x: usize, y: usize, f: usize) -> ([u8; 3], u16, u8) {
    ([(3*x + y + 13*f) as u8, (x + 5*y + 31*f) as u8, (7*x + 11*y + 97*f) as u8], (191*x + 103*y + 1007*f) as u16, ((2*x + y + f) % 4) as u8)
}

#[test]
fn frame_trees_and_openings_match_circomlib() {
    let vectors: Vectors<FrameCase> = serde_json::from_str(include_str!("../../../test-vectors/protocol/trees.json")).unwrap();
    for v in vectors.cases {
        let kind = match v.kind { 1 => TreeKind::Rgb, 2 => TreeKind::Depth, 3 => TreeKind::Frame, _ => panic!("tree kind") };
        let mut rgb = Vec::new();
        let mut depth = Vec::new();
        let mut conf = Vec::new();
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let (r, d, c) = pixel(x, y, v.frame.into());
                if kind != TreeKind::Depth { rgb.extend(r); }
                if kind != TreeKind::Rgb { depth.push(d); conf.push(c); }
            }
        }
        let tree = FrameTree::build(kind, v.frame, &rgb, &depth, &conf).unwrap();
        let root = fr(v.root);
        assert_eq!(tree.root(), root);
        for p in v.openings {
            assert_eq!(p.slot, leaf_slot(p.tile, p.bx, p.by));
            let (tx, ty) = tile_xy(p.tile);
            let (x, y) = (usize::from(tx)*32 + usize::from(p.bx)*4, usize::from(ty)*32 + usize::from(p.by)*4);
            let (mut rgb, mut d, mut c) = ([0; 48], [0; 16], [0; 16]);
            for k in 0..16 {
                let (r, depth, conf) = pixel(x + k%4, y + k/4, v.frame.into());
                rgb[3*k..3*k+3].copy_from_slice(&r); d[k] = depth; c[k] = conf;
            }
            let (r, dep) = (pack_rgb(&rgb), pack_depth(&d, &c));
            assert_eq!([r.0, r.1, dep.0, dep.1], p.packed.map(fr));
            let leaf = match kind {
                TreeKind::Rgb => rgb_leaf(v.frame, p.slot, r),
                TreeKind::Depth => depth_leaf(v.frame, p.slot, dep),
                TreeKind::Frame => frame_leaf(v.frame, p.slot, r, dep),
            };
            assert_eq!(leaf, fr(p.leaf));
            let mut opening = tree.opening(p.slot).unwrap();
            assert_eq!(opening.siblings, p.siblings.map(|s| s.map(fr)));
            assert_eq!(opening.root_from_leaf(kind, v.frame, leaf).unwrap(), root);
            opening.siblings[0].swap(0, 1);
            assert_ne!(opening.root_from_leaf(kind, v.frame, leaf).unwrap(), root);
        }
        assert!(tree.opening(leaf_slot(48, 0, 0)).is_err());
        assert!(tree.opening(4096).is_err());
        assert!(tree.opening(u16::MAX).is_err());
    }
}

#[test]
fn components_and_evidence_root_match_circomlib() {
    let vectors: Vectors<CommitCase> = serde_json::from_str(include_str!("../../../test-vectors/protocol/commitments.json")).unwrap();
    for v in vectors.cases {
        let roots = v.tree_roots.map(fr);
        let salts = v.salts.map(fr);
        let elements = v.elements.map(|s| s.into_iter().map(fr).collect::<Vec<_>>());
        let components = std::array::from_fn(|i| match i {
            0 => roots[0],
            1..=3 => tree_component(i as u8, salts[i-1], roots[i]),
            4..=8 => scalar_component(i as u8, salts[i-1], &elements[i-4]),
            _ => pad_component(i as u8),
        });
        assert_eq!(components, v.components.map(fr));
        assert_eq!(evidence_root(&components), fr(v.root));
        assert_ne!(scalar_component(4, salts[3], &[]), scalar_component(4, salts[3], &[Fr::from(0u8)]));
    }
}

#[test]
fn invalid_tree_inputs_are_rejected_before_hashing() {
    let depth = vec![1; WIDTH*HEIGHT];
    let mut conf = vec![1; WIDTH*HEIGHT];
    assert!(matches!(FrameTree::build(TreeKind::Depth, 0, &[], &depth, &conf), Err(TreeError::Frame)));
    assert!(matches!(FrameTree::build(TreeKind::Depth, 1, &[0], &depth, &conf), Err(TreeError::Plane("rgb"))));
    assert!(matches!(FrameTree::build(TreeKind::Depth, 1, &[], &depth[..100], &conf), Err(TreeError::Plane("depth"))));
    conf[4] = 4;
    assert!(matches!(FrameTree::build(TreeKind::Depth, 1, &[], &depth, &conf), Err(TreeError::Confidence)));
}
