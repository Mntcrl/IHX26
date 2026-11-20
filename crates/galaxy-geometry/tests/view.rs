//! `build_view` openings, §20 JSON round trip and fail-closed input checks.

use std::path::Path;
use std::sync::LazyLock;

use galaxy_evidence::Evidence;
use galaxy_geometry::synthetic::{synthesize, synthetic_challenge, Scenario};
use galaxy_geometry::{build_view, evaluate, try_evaluate, GeometryView, PolicyReport, ViewError};
use galaxy_protocol::policy::Policy;
use serde_json::Value;

const SEED: u64 = 0;

fn v1m() -> Policy {
    Policy::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../policies/galaxy-pc-v1m.json")).expect("v1m policy loads")
}

/// Rendered once; every test clones it.
static ROOM: LazyLock<(Evidence, GeometryView)> = LazyLock::new(|| {
    let evidence = synthesize(Scenario::Room, SEED, &v1m()).expect("synthetic room");
    let view = build_view(&evidence, &synthetic_challenge(SEED)).expect("view");
    (evidence, view)
});

fn room() -> (Evidence, GeometryView) {
    ROOM.clone()
}

fn json(view: &GeometryView) -> Value {
    serde_json::to_value(view).expect("serialize")
}

fn from_json(value: Value) -> Result<GeometryView, serde_json::Error> {
    serde_json::from_value(value)
}

#[test]
fn openings_match_the_challenge_and_frames() {
    let (evidence, view) = room();
    let challenge = synthetic_challenge(SEED);
    assert!(view.expected.is_none());
    assert_eq!(view.tiles, challenge.tiles);
    assert_eq!(view.hyp_idx, challenge.hyp);
    for b in 0..64 {
        let (slot, q) = (b / 4, b % 4);
        let tile = challenge.tiles[slot];
        let (bx, by) = challenge.picks[slot].blocks[q];
        assert_eq!((view.blocks[b].bx, view.blocks[b].by), (bx, by));
        assert_eq!(view.blk_u0[b], 32 * u16::from(tile % 8) + 4 * u16::from(bx));
        assert_eq!(view.blk_v0[b], 32 * u16::from(tile / 8) + 4 * u16::from(by));
        for k in 0..16 {
            let pixel = 256 * (usize::from(view.blk_v0[b]) + k / 4) + usize::from(view.blk_u0[b]) + k % 4;
            assert_eq!(view.blk_d[b][k], evidence.frames[1].depth_mm[pixel]);
            assert_eq!(view.blk_c[b][k], evidence.frames[1].conf[pixel]);
            assert_eq!(view.blk_g[b][k], evidence.frames[1].rgb[3 * pixel + 1]);
        }
    }
    for c in 0..32 {
        let q = [0, 3][c % 2];
        assert_eq!((view.tmp_ix[c], view.tmp_iy[c]), challenge.picks[c / 2].temporal[q]);
    }
    let mut inside = 0;
    for (p, pair) in view.pairs.iter().enumerate() {
        assert_eq!((pair.slot, pair.q, pair.frame), ((p / 4) as u8, [0, 3][(p / 2) % 2], [0, 2][p % 2]));
        let frame = &evidence.frames[usize::from(pair.frame)];
        if pair.inb {
            inside += 1;
            let u: i64 = pair.uj.as_deref().expect("uj").parse().unwrap();
            let v: i64 = pair.vj.as_deref().expect("vj").parse().unwrap();
            assert_eq!((i64::from(view.pr_bx[p]), i64::from(view.pr_by[p])), (u >> 2, v >> 2));
        } else {
            assert_eq!((view.pr_bx[p], view.pr_by[p]), (0, 0));
        }
        for k in 0..16 {
            let pixel = 256 * (4 * usize::from(view.pr_by[p]) + k / 4) + 4 * usize::from(view.pr_bx[p]) + k % 4;
            assert_eq!(view.pr_d[p][k], frame.depth_mm[pixel]);
            assert_eq!(view.pr_r[p][k], frame.rgb[3 * pixel]);
        }
    }
    assert!(inside > 32, "hand-held room keeps most projections inside the raster");
}

#[test]
fn json_round_trip_uses_template_names() {
    let (_, view) = room();
    let view = view.with_expected(&v1m());
    let value = json(&view);
    for key in ["blk_r", "blk_d", "tmp_ix", "hyp_idx", "pr_bx", "R", "T", "imu_dt", "imu_w", "g0", "a_peak", "expected"] {
        assert!(value.get(key).is_some(), "{key}");
    }
    assert!(value["pairs"].as_array().unwrap().iter().all(|p| p["uj"].is_string() || p["uj"].is_null()));
    assert!(value["expected"].get("V_all").is_some() && value["expected"].get("N_pok").is_some());
    let back = from_json(value).expect("round trip");
    assert!(back == view);
    assert_eq!(back.expected, Some(evaluate(&view, &v1m())));
}

#[test]
fn malformed_json_is_rejected() {
    let (_, view) = room();
    let mut extra = json(&view);
    extra["unknown"] = Value::from(1);
    assert!(from_json(extra).is_err());
    let mut version = json(&view);
    version["version"] = Value::from(2);
    assert!(from_json(version).is_err());
    let mut short = json(&view);
    short["blk_d"].as_array_mut().unwrap().pop();
    assert!(from_json(short).is_err());
    let mut missing = json(&view);
    missing["pairs"][0].as_object_mut().unwrap().remove("uj");
    assert!(from_json(missing).is_err());
    let mut depth = json(&view);
    depth["blk_d"][0][0] = Value::from(65536);
    assert!(from_json(depth).is_err());
}

fn rejected(edit: impl FnOnce(&mut GeometryView)) -> ViewError {
    let (_, mut view) = room();
    edit(&mut view);
    assert_eq!(evaluate(&view, &v1m()), PolicyReport::default(), "evaluate fails closed");
    try_evaluate(&view, &v1m()).expect_err("view must be rejected")
}

#[test]
fn out_of_domain_views_fail_closed() {
    assert_eq!(rejected(|v| v.fx[0] = 1 << 26), ViewError::ViewDomain);
    assert_eq!(rejected(|v| v.cy[2] = -1), ViewError::ViewDomain);
    assert_eq!(rejected(|v| v.r[1][4] = i64::from(i32::MAX) + 1), ViewError::ViewDomain);
    assert_eq!(rejected(|v| v.t[0][0] = i64::from(i32::MIN) - 1), ViewError::ViewDomain);
    assert_eq!(rejected(|v| v.ts[1] = 1 << 48), ViewError::ViewDomain);
    assert_eq!(rejected(|v| v.track[0] = 4), ViewError::ViewDomain);
    assert_eq!(rejected(|v| v.blk_c[3][5] = 4), ViewError::ViewDomain);
    assert_eq!(rejected(|v| v.pr_c[3][5] = 4), ViewError::ViewDomain);
    assert_eq!(rejected(|v| v.tmp_ix[7] = 3), ViewError::ViewDomain);
    assert_eq!(rejected(|v| v.hyp_idx[4][0] = 128), ViewError::ViewDomain);
    assert_eq!(rejected(|v| v.tiles.swap(0, 1)), ViewError::ViewDomain);
    assert_eq!(rejected(|v| v.tiles[8] = v.tiles[0]), ViewError::ViewDomain);
    assert_eq!(rejected(|v| v.blk_u0[9] += 4), ViewError::ViewDomain);
    assert_eq!(rejected(|v| v.blocks[5].q = 0), ViewError::ViewDomain);
    assert_eq!(rejected(|v| v.blocks[0].bx = 4), ViewError::ViewDomain);
}

#[test]
fn temporal_opening_must_follow_the_projection() {
    let (_, view) = room();
    let p = view.pairs.iter().position(|pair| pair.inb).expect("one projection inside the raster");
    assert_eq!(rejected(|v| v.pr_bx[p] ^= 1), ViewError::PairOpening);
    if let Some(p) = view.pairs.iter().position(|pair| !pair.inb) {
        assert_eq!(rejected(|v| v.pr_by[p] = 1), ViewError::PairOpening);
    }
}

#[test]
fn build_view_rejects_bad_evidence() {
    let challenge = synthetic_challenge(SEED);
    let (evidence, _) = room();
    let mut conf = evidence.clone();
    conf.frames[2].conf[100] = 4;
    assert_eq!(build_view(&conf, &challenge).err(), Some(ViewError::ViewDomain));
    let mut shape = evidence.clone();
    shape.frames[0].depth_mm.pop();
    assert_eq!(build_view(&shape, &challenge).err(), Some(ViewError::EvidenceShape));
    let mut imu = evidence.clone();
    imu.imu.windows[1].pop();
    assert_eq!(build_view(&imu, &challenge).err(), Some(ViewError::EvidenceShape));
    let mut focal = evidence.clone();
    focal.camera[1].fx = 1 << 26;
    assert_eq!(build_view(&focal, &challenge).err(), Some(ViewError::ViewDomain));
    let mut rotation = evidence;
    rotation.camera[2].r[0] = i64::from(i32::MAX) + 1;
    assert_eq!(build_view(&rotation, &challenge).err(), Some(ViewError::ViewDomain));
}

#[test]
fn synthetic_room_passes_and_is_deterministic() {
    let (_, view) = room();
    let report = try_evaluate(&view, &v1m()).expect("in domain");
    assert!(report.pass, "{report:?}");
    let (_, again) = room();
    assert!(again == view);
}
