//! Ingest of the private device captures in `fixtures/real/capture-ios/` (gitignored, spec §19/§21).
//! The test is skipped when the directory is absent (CI, other machines). It prints only
//! quantized aggregate statistics, never pixel, depth or IMU sample values.

#![cfg(not(target_arch = "wasm32"))]

use galaxy_evidence::{
    bundle::{self, EvidenceBundle},
    commit,
    ingest::from_gxrw,
    Evidence, ImageProfile, Salts,
};
use galaxy_image::decode_any;
use std::path::PathBuf;

fn captures() -> Vec<PathBuf> {
    let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/real/capture-ios"));
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "gxrw"))
        .collect();
    paths.sort();
    paths
}

/// Euclidean distance between two camera positions (µm), reported in whole millimetres.
fn baseline_mm(a: [i64; 3], b: [i64; 3]) -> i64 {
    let d2: f64 = a.iter().zip(&b).map(|(x, y)| ((x - y) as f64).powi(2)).sum();
    (d2.sqrt() / 1000.0).round() as i64
}

fn summary(e: &Evidence) -> String {
    let valid: Vec<String> = e
        .frames
        .iter()
        .map(|f| {
            let valid = f.depth_mm.iter().filter(|&&d| d != 0).count();
            format!("{:.2}", valid as f64 / f.depth_mm.len() as f64)
        })
        .collect();
    let high: Vec<String> = e
        .frames
        .iter()
        .map(|f| format!("{:.2}", f.conf.iter().filter(|&&c| c == 2).count() as f64 / f.conf.len() as f64))
        .collect();
    let c = &e.camera[1];
    let q = |v: i64| (v as f64 / 65_536.0 * 10.0).round() / 10.0;
    let imu: Vec<usize> = e.imu.windows.iter().map(|w| w.iter().filter(|s| s.dt_us != 0).count()).collect();
    format!(
        "depth_valid={valid:?} conf_high={high:?} intrinsics_a(fx,fy,cx,cy)=({}, {}, {}, {}) \
         baselines_mm(minus,plus)=({}, {}) offsets_ms={:?} track={:?} imu_samples={imu:?} \
         a_peak_mg_bucket={} matrix_code={} model_code={}",
        q(c.fx),
        q(c.fy),
        q(c.cx),
        q(c.cy),
        baseline_mm(e.camera[0].t, e.camera[1].t),
        baseline_mm(e.camera[2].t, e.camera[1].t),
        e.meta.frame_offsets_ms,
        e.camera.map(|c| c.track),
        e.imu.a_peak_mg / 50 * 50,
        e.meta.ycbcr_matrix_code,
        e.meta.device_model_code,
    )
}

#[test]
fn real_device_captures_ingest() {
    let paths = captures();
    if paths.is_empty() {
        eprintln!("real captures: fixtures/real/capture-ios absent, skipped");
        return;
    }
    for (index, path) in paths.iter().enumerate() {
        let bytes = std::fs::read(path).unwrap();
        let png = from_gxrw(&bytes, ImageProfile::PngV1).unwrap_or_else(|e| panic!("capture #{}: {e}", index + 1));
        let jpeg = from_gxrw(&bytes, ImageProfile::JpegV1).unwrap_or_else(|e| panic!("capture #{}: {e}", index + 1));
        png.validate().unwrap();
        jpeg.validate().unwrap();
        assert_eq!(decode_any(&png.image.encoded).unwrap().raster, png.image.rgb);
        assert_eq!(decode_any(&jpeg.image.encoded).unwrap().raster, jpeg.image.rgb);
        assert!(png.frames[0] == jpeg.frames[0] && png.frames[2] == jpeg.frames[2]);
        assert!(png.camera == jpeg.camera && png.imu == jpeg.imu);
        let salts = Salts::random().unwrap();
        let c = commit(&png, &salts);
        assert_eq!(c.components[0], c.rgb_root);
        let original = EvidenceBundle { evidence: jpeg, salts, capture_state: Vec::new() };
        let encoded = bundle::encode(&original).unwrap();
        assert!(bundle::decode(&encoded).unwrap() == original);
        eprintln!(
            "real capture #{} (png {} B, jpeg {} B, bundle {} B): {}",
            index + 1,
            png.image.encoded.len(),
            original.evidence.image.encoded.len(),
            encoded.len(),
            summary(&png)
        );
    }
}
