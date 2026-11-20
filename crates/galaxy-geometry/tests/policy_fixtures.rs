//! The checked-in `test-vectors/policy/` fixtures agree with the reference
//! engine and are exactly what the generator produces.

use std::path::{Path, PathBuf};

use galaxy_geometry::fixtures::{coefficient_policy, render_files, COEFFICIENT_DIR, COEFFICIENT_POLICY_NAME};
use galaxy_geometry::{try_evaluate, GeometryView};
use galaxy_protocol::policy::Policy;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn v1m() -> Policy {
    Policy::load(root().join("policies/galaxy-pc-v1m.json")).expect("v1m policy loads")
}

fn fixtures_dir() -> PathBuf {
    root().join("test-vectors/policy")
}

fn load_view(path: &Path) -> GeometryView {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn default_fixtures_match_reference_engine() {
    let policy = v1m();
    let mut paths: Vec<_> = std::fs::read_dir(fixtures_dir()).expect("fixture dir").map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    paths.sort();
    assert!(paths.len() >= 12, "only {} fixtures", paths.len());
    let (mut passing, mut offscreen) = (0, 0);
    let mut flags = [[false; 2]; 6];
    for path in &paths {
        let view = load_view(path);
        let expected = view.expected.unwrap_or_else(|| panic!("{} lacks expected", path.display()));
        let report = try_evaluate(&view, &policy).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(report, expected, "{}", path.display());
        passing += usize::from(report.pass);
        offscreen += usize::from(view.pairs.iter().any(|p| !p.inb));
        for (i, b) in [report.quality_ok, report.geometry_ok, report.rgb_depth_ok, report.temporal_ok, report.motion_ok, report.pass]
            .into_iter().enumerate()
        {
            flags[i][usize::from(b)] = true;
        }
    }
    assert!(passing > 0 && offscreen > 0);
    assert!(flags.iter().all(|f| f[0] && f[1]), "every predicate must appear both true and false: {flags:?}");
}

#[test]
fn coefficient_fixture_uses_isolated_policy() {
    let dir = fixtures_dir().join(COEFFICIENT_DIR);
    let test_policy = Policy::load(dir.join("policy-test-dmin1.json")).expect("test policy loads");
    assert_eq!(test_policy, coefficient_policy(&v1m()));
    assert_eq!(test_policy.name, COEFFICIENT_POLICY_NAME);
    assert_ne!(test_policy.id().unwrap(), v1m().id().unwrap());
    let view = load_view(&dir.join("view.json"));
    assert_eq!(Some(try_evaluate(&view, &test_policy).unwrap()), view.expected);
}

#[test]
fn checked_in_files_are_generator_output() {
    for (name, bytes) in render_files(&v1m()).expect("fixtures render") {
        let path = fixtures_dir().join(&name);
        let current = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(current == bytes, "{name} differs from generator output; regenerate with geometry-tool fixtures");
    }
}
