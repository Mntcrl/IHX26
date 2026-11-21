//! Reproducible calibration on the private, reviewed GXRW corpus. Only policy
//! values and class-level counts leave this command; captures/views stay local.

use std::collections::BTreeMap;
use std::path::Path;

use galaxy_evidence::Evidence;
use serde::Deserialize;

use super::{build_view, ingest_gxrw, print_json, try_evaluate, Challenge, Error, GeometryView, Policy, Profile};
use galaxy_geometry::synthetic::{self, suite::{self, ScenarioOutcome}, Scenario};

const CHALLENGES: u8 = 32;
const SCENE_GAP_MS: u64 = 120_000;
const CLASSES: [&str; 10] = [
    "genuine-3d", "genuine-scene-with-screen", "dynamic", "screen-recapture", "printed-photo",
    "flat-wall", "screen-on-flat-surface", "genuine-3d-low-motion", "reflective", "no-motion",
];
const PRINT: usize = 4;
const LIMITS: [(usize, usize); 4] = [(3, 2), (PRINT, 2), (5, 5), (6, 5)];

#[derive(Deserialize)]
struct Manifest {
    entries: Vec<Entry>,
}
#[derive(Deserialize)]
struct Entry {
    include: bool,
    capture_id: String,
    claimed_time_ms: Option<u64>,
    curated_class: Option<String>,
}

struct Capture {
    valid_depth: bool,
    views: Vec<GeometryView>,
}

type Corpus = Vec<Vec<Capture>>;

fn scenes(entries: &[&Entry]) -> Vec<std::ops::Range<usize>> {
    let mut starts = vec![0];
    starts.extend(entries.windows(2).enumerate().filter_map(|(i, pair)| {
        (pair[1].claimed_time_ms.unwrap() - pair[0].claimed_time_ms.unwrap() > SCENE_GAP_MS).then_some(i + 1)
    }));
    starts.push(entries.len());
    starts.windows(2).filter(|p| p[0] < p[1]).map(|p| p[0]..p[1]).collect()
}

fn held_out(sizes: &[usize]) -> Vec<usize> {
    if sizes.len() <= 1 { return Vec::new(); }
    let n = ((3 * sizes.len() + 5) / 10).clamp(1, sizes.len() - 1);
    // Hold out the required number of whole scenes; among those subsets,
    // approximate 30% of captures using only metadata. Earliest indices win ties.
    let mut subsets = BTreeMap::from([((0, 0), Vec::<usize>::new())]);
    for (i, size) in sizes.iter().enumerate() {
        let added = subsets.iter().filter(|((count, _), _)| *count < n)
            .map(|(&(count, total), indices)| {
                let mut indices = indices.clone();
                indices.push(i);
                ((count + 1, total + size), indices)
            }).collect::<Vec<_>>();
        for (key, indices) in added {
            use std::collections::btree_map::Entry;
            match subsets.entry(key) {
                Entry::Vacant(slot) => { slot.insert(indices); }
                Entry::Occupied(mut slot) if &indices < slot.get() => { slot.insert(indices); }
                Entry::Occupied(_) => {}
            }
        }
    }
    let total = sizes.iter().sum::<usize>();
    subsets.into_iter().filter(|((count, _), _)| *count == n)
        .min_by(|((_, a), ai), ((_, b), bi)| {
            (10 * a).abs_diff(3 * total).cmp(&(10 * b).abs_diff(3 * total)).then_with(|| ai.cmp(bi))
        })
        .expect("there is at least one subset").1
}

fn valid_depth(evidence: &Evidence) -> bool {
    // Fixed baseline (not the candidate) so moving d_min cannot remove a print
    // from the denominator. Match the baseline minimum valid fractions.
    let frame = &evidence.frames[1];
    let (mut all, mut central) = (0, 0);
    for (i, (&d, &c)) in frame.depth_mm.iter().zip(&frame.conf).enumerate() {
        if (200..=5000).contains(&d) && c >= 1 {
            all += 1;
            central += usize::from((64..192).contains(&(i % 256)) && (32..160).contains(&(i / 256)));
        }
    }
    all >= 256 * 192 / 2 && central >= 128 * 128 / 2
}

fn load(dir: &Path, test: bool, fitting: bool) -> Result<Corpus, Error> {
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(dir.join("manifest.json"))?)?;
    if manifest.entries.iter().any(|e| e.include && (e.claimed_time_ms.is_none() ||
        !e.curated_class.as_deref().is_some_and(|c| CLASSES.contains(&c)))) {
        return Err("included capture has no timestamp or has an unknown class".into());
    }
    let mut corpus = Vec::new();
    for (class, name) in CLASSES.into_iter().enumerate() {
        let mut entries: Vec<_> = manifest.entries.iter().filter(|e| e.include && e.curated_class.as_deref() == Some(name)).collect();
        entries.sort_unstable_by(|a, b| (a.claimed_time_ms, &a.capture_id).cmp(&(b.claimed_time_ms, &b.capture_id)));
        let ranges = scenes(&entries);
        let sizes: Vec<_> = ranges.iter().map(std::ops::Range::len).collect();
        let held = held_out(&sizes);
        if !fitting {
            let summary = sizes.iter().enumerate().map(|(i, n)| format!("{n}{}", if held.contains(&i) { "*" } else { "" })).collect::<Vec<_>>();
            println!("| {name} | {} |", summary.join(", "));
        }
        let mut captures = Vec::new();
        if !fitting || class <= 6 {
            for (_, range) in ranges.into_iter().enumerate().filter(|(i, _)| held.contains(i) == test) {
                for entry in &entries[range] {
                    let mut id = [0; 32];
                    id[..16].copy_from_slice(b"GALAXY/STUDY/v1\0");
                    hex::decode_to_slice(entry.capture_id.replace('-', ""), &mut id[16..])
                        .map_err(|_| "invalid manifest capture identifier")?;
                    let file = dir.join("classes").join(name).join(format!("{}.gxrw", entry.capture_id));
                    let evidence = ingest_gxrw(&file, Profile::Png).map_err(|_| "could not ingest a study capture")?;
                    let views = (0..CHALLENGES).map(|k| {
                        let ledger = [k; 32];
                        let challenge = Challenge::derive(1, &id, 5, &ledger);
                        build_view(&evidence, &challenge)
                    }).collect::<Result<_, _>>()?;
                    captures.push(Capture { valid_depth: valid_depth(&evidence), views });
                }
            }
        }
        corpus.push(captures);
    }
    Ok(corpus)
}

pub(super) fn measure(dir: &Path, test: bool, policy: &Policy) -> Result<(), Error> {
    println!("| class | chronological scene sizes (* held out) |\n|---|---|");
    let corpus = load(dir, test, false)?;
    println!("\n| class | captures | with ≥1 PASS | with all 32 PASS |\n|---|---:|---:|---:|");
    let mut outcomes = Vec::new();
    for (class, captures) in corpus.iter().enumerate() {
        let mut add = |label: &str, valid: Option<bool>| -> Result<(), Error> {
            let reports = captures.iter().filter(|c| valid.is_none_or(|valid| c.valid_depth == valid))
                .flat_map(|c| &c.views).map(|v| try_evaluate(v, policy)).collect::<Result<Vec<_>, _>>()?;
            let n = usize::from(CHALLENGES);
            let any = reports.chunks_exact(n).filter(|group| group.iter().any(|r| r.pass)).count();
            let all = reports.chunks_exact(n).filter(|group| group.iter().all(|r| r.pass)).count();
            println!("| {label} | {} | {any} | {all} |", reports.len() / n);
            let label = format!("{label} ({} captures)", reports.len() / n);
            outcomes.push(ScenarioOutcome { label, reports });
            Ok(())
        };
        add(CLASSES[class], None)?;
        if class == PRINT {
            add("printed-photo (baseline-valid depth)", Some(true))?;
            add("printed-photo (insufficient baseline-valid depth)", Some(false))?;
        }
    }
    println!("\n{}", suite::markdown(&outcomes));
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Score {
    excess_ppm: usize,
    missed_genuine: usize,
    changes: usize,
}

fn score(corpus: &Corpus, candidate: &Policy, baseline: &Policy) -> Result<Score, Error> {
    let mut passed = [0; CLASSES.len()];
    let mut totals = [0; CLASSES.len()];
    for (class, captures) in corpus.iter().enumerate() {
        for capture in captures.iter().filter(|c| class != PRINT || c.valid_depth) {
            totals[class] += capture.views.len();
            for view in &capture.views {
                passed[class] += usize::from(try_evaluate(view, candidate)?.pass);
            }
        }
    }
    let excess_ppm = LIMITS.iter().map(|&(class, limit)| {
        if totals[class] == 0 { return Err("no evaluable training captures for an objective class"); }
        Ok((1_000_000 * passed[class] / totals[class]).saturating_sub(10_000 * limit))
    }).sum::<Result<usize, _>>()?;
    let missed_genuine = (0..3).map(|class| totals[class] - passed[class]).sum();
    let changes = grid().iter().filter(|field| (field.get)(candidate) != (field.get)(baseline)).count();
    Ok(Score { excess_ppm, missed_genuine, changes })
}

const GUARDED_SCENARIOS: [Scenario; 8] = [
    Scenario::CentreMonitor, Scenario::FrontalMonitor, Scenario::TiltedMonitor,
    Scenario::CurvedMonitor, Scenario::Print, Scenario::FlatWall,
    Scenario::DepthSubstitution, Scenario::SwappedFrames,
];

struct SyntheticCap {
    views: Vec<GeometryView>,
    max_passes: usize,
}

impl SyntheticCap {
    fn build(baseline: &Policy) -> Result<Vec<Self>, Error> {
        let mut caps = Vec::with_capacity(GUARDED_SCENARIOS.len() * 2);
        // The generator uses only the policy's device-to-camera matrix, which
        // the fit does not vary. Reuse the same views for every candidate.
        for scenario in GUARDED_SCENARIOS {
            for profile in [Profile::Png, Profile::Jpeg] {
                let mut views = Vec::with_capacity(20);
                let mut max_passes = 0;
                for seed in 0..20 {
                    let evidence = synthetic::synthesize_with_profile(scenario, seed, baseline, profile.into())?;
                    let view = build_view(&evidence, &synthetic::synthetic_challenge(seed))?;
                    max_passes += usize::from(try_evaluate(&view, baseline)?.pass);
                    views.push(view);
                }
                // A count cannot regress when the baseline already passes all seeds.
                if max_passes < views.len() {
                    caps.push(Self { views, max_passes });
                }
            }
        }
        Ok(caps)
    }

    fn accepts(caps: &[Self], candidate: &Policy) -> Result<bool, Error> {
        for cap in caps {
            let mut passes = 0;
            for view in &cap.views {
                passes += usize::from(try_evaluate(view, candidate)?.pass);
                if passes > cap.max_passes { return Ok(false); }
            }
        }
        Ok(true)
    }
}

fn consider(
    corpus: &Corpus,
    caps: &[SyntheticCap],
    candidate: &Policy,
    baseline: &Policy,
    best: &mut Option<(Score, Policy)>,
) -> Result<(), Error> {
    if candidate.validate().is_err() { return Ok(()); }
    let result = score(corpus, candidate, baseline)?;
    if best.as_ref().is_none_or(|(rank, _)| result < *rank) && SyntheticCap::accepts(caps, candidate)? {
        *best = Some((result, candidate.clone()));
    }
    Ok(())
}

struct Field {
    get: fn(&Policy) -> i64,
    set: fn(&mut Policy, i64),
    values: &'static [i64],
    fixed: bool,
}

fn grid() -> [Field; 22] {
    macro_rules! fields {
        ($($group:ident.$key:ident: $fixed:literal [$($v:expr),+]);+ $(;)?) => {[$(
            Field { get: |p| p.$group.$key, set: |p, v| p.$group.$key = v, values: &[$($v),+], fixed: $fixed },
        )+]};
    }
    fields![
        lidar.d_min_mm: false [200, 225, 250, 275, 300, 325, 350, 400];
        lidar.eps_log2: false [25, 26, 27, 28];
        lidar.sep_log2: false [27, 28, 29];
        lidar.plane_global_max_pct: true [70, 75, 80, 85, 90];
        lidar.plane_central_max_pct: true [65, 70, 75, 80, 85, 90];
        lidar.tile_out_min: true [3, 4, 5, 6, 8];
        lidar.nonplanar_tiles_min: true [4, 6, 8, 10, 12];
        lidar.nonplanar_tiles_c_min: true [2, 3, 4, 5, 6];
        lidar.strong_min_pct: false [5, 10, 15, 20, 25, 30];
        lidar.strong_tiles_min: false [2, 3, 4, 6, 8];
        rgb_depth.disc_pct: false [2, 3, 4, 5, 6];
        rgb_depth.grad_min_y: false [1280, 1792, 2048, 2560, 3072, 3840];
        rgb_depth.disc_min: false [3, 4, 5, 6, 8, 10];
        rgb_depth.edge_match_pct: false [30, 40, 45, 50, 55, 60, 70];
        temporal.occ_pm: false [45, 50, 65, 80];
        temporal.dcons_pm: false [20, 25, 30, 35, 40];
        temporal.photo_tol_y: false [2560, 3840, 5120, 6400, 7680];
        temporal.occ_max_pct: false [20, 30, 40, 50];
        temporal.dok_min_pct: false [60, 65, 70, 75, 80, 85];
        temporal.pok_min_pct: false [50, 55, 60, 65, 70, 75, 80];
        motion.gyro_tol_urad: false [2000, 3000, 5000, 10000, 20000];
        motion.rot_max_q30: false [107374182, 214748365, 429496730, 751619277];
    ]
}

pub(super) fn fit(dir: &Path, baseline: &Policy, unconstrained: bool) -> Result<(), Error> {
    let caps = if unconstrained { Vec::new() } else { SyntheticCap::build(baseline)? };
    let corpus = load(dir, false, true)?;
    let fields = grid();
    // Begin from every d_min/eps pair and the baseline strong band. Choose the
    // best starting policy per distance, then refine the three best distances.
    let mut seeds = Vec::new();
    for &distance in fields[0].values {
        let mut candidate = baseline.clone();
        candidate.lidar.d_min_mm = distance;
        let mut best = None;
        consider(&corpus, &caps, &candidate, baseline, &mut best)?;
        for &eps in fields[1].values {
            if eps == baseline.lidar.eps_log2 { continue; }
            candidate.lidar.eps_log2 = eps;
            consider(&corpus, &caps, &candidate, baseline, &mut best)?;
        }
        seeds.extend(best);
    }
    seeds.sort_by_key(|(rank, _)| *rank);
    let mut winner = (score(&corpus, baseline, baseline)?, baseline.clone());
    for (mut rank, mut current) in seeds.into_iter().take(3) {
        for _ in 0..4 {
            let previous = rank;
            for field in fields.iter().filter(|field| unconstrained || !field.fixed) {
                let mut candidate = current.clone();
                let mut best = Some((rank, current));
                for &v in field.values {
                    if v == (field.get)(&best.as_ref().expect("initial candidate").1) { continue; }
                    (field.set)(&mut candidate, v);
                    consider(&corpus, &caps, &candidate, baseline, &mut best)?;
                }
                (rank, current) = best.expect("initial candidate");
            }
            if rank == previous { break; }
        }
        if rank < winner.0 { winner = (rank, current); }
    }
    eprintln!("training score: {:?}", winner.0);
    print_json(&winner.1)
}

#[cfg(test)]
mod tests {
    use super::{build_view, held_out, synthetic, Policy, Scenario, SyntheticCap};

    #[test]
    fn whole_scene_partition_is_deterministic() {
        assert!(held_out(&[]).is_empty());
        assert!(held_out(&[6]).is_empty());
        assert_eq!(held_out(&[2, 1, 1, 10]), [0]);
        assert_eq!(held_out(&[2, 5, 3, 9, 2, 2, 2, 19]), [1, 3]);
        assert_eq!(held_out(&[1, 1, 1]), [0]);
    }

    #[test]
    fn synthetic_cap_accepts_ties_but_not_increased_passes() {
        let policy = Policy::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../policies/galaxy-pc-v1m.json")).unwrap();
        let evidence = synthetic::synthesize(Scenario::Room, 0, &policy).unwrap();
        let view = build_view(&evidence, &synthetic::synthetic_challenge(0)).unwrap();
        let mut caps = [SyntheticCap { views: vec![view], max_passes: 1 }];
        assert!(SyntheticCap::accepts(&caps, &policy).unwrap());
        caps[0].max_passes = 0;
        assert!(!SyntheticCap::accepts(&caps, &policy).unwrap());
    }
}
