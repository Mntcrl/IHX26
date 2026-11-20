//! Scenario suite over synthetic captures: per scenario, outcome counts over a
//! seed range and per-predicate failure counts (docs/experiments/synthetic-v1m.md).
//! Each capture goes through the full synthetic path (render, canonical image
//! encode/decode, `build_view` with the synthetic challenge, `evaluate`).

use galaxy_evidence::ImageProfile;
use galaxy_protocol::policy::Policy;

use super::{synthesize_with_profile, synthetic_challenge, Scenario, SyntheticError};
use crate::{build_view, try_evaluate, PolicyReport, Statistics, ViewError};

#[derive(Debug, thiserror::Error)]
pub enum SuiteError {
    #[error(transparent)]
    Synthetic(#[from] SyntheticError),
    #[error(transparent)]
    View(#[from] ViewError),
}

pub const PREDICATES: [&str; 5] = ["quality", "geometry", "rgb_depth", "temporal", "motion"];

fn predicates(r: &PolicyReport) -> [bool; 5] {
    [r.quality_ok, r.geometry_ok, r.rgb_depth_ok, r.temporal_ok, r.motion_ok]
}

pub struct ScenarioOutcome {
    /// Scenario name, or capture category for real-capture calibration.
    pub label: String,
    pub reports: Vec<PolicyReport>,
}

impl ScenarioOutcome {
    pub fn passes(&self) -> usize {
        self.reports.iter().filter(|r| r.pass).count()
    }

    /// Number of seeds on which each predicate is false.
    pub fn failures(&self) -> [usize; 5] {
        std::array::from_fn(|i| self.reports.iter().filter(|r| !predicates(r)[i]).count())
    }

    /// Number of seeds on which the predicate is the only false one.
    pub fn sole_failures(&self) -> [usize; 5] {
        std::array::from_fn(|i| self.reports.iter().filter(|r| {
            let p = predicates(r);
            !p[i] && p.iter().filter(|&&b| !b).count() == 1
        }).count())
    }

    fn median(&self, f: impl Fn(&Statistics) -> u16) -> u16 {
        let mut v: Vec<u16> = self.reports.iter().map(|r| f(&r.stats)).collect();
        v.sort_unstable();
        v.get(v.len() / 2).copied().unwrap_or(0)
    }
}

pub fn run(policy: &Policy, scenarios: &[Scenario], seeds: std::ops::Range<u64>, profile: ImageProfile) -> Result<Vec<ScenarioOutcome>, SuiteError> {
    scenarios.iter().map(|&scenario| {
        let reports = seeds.clone().map(|seed| {
            let evidence = synthesize_with_profile(scenario, seed, policy, profile)?;
            let view = build_view(&evidence, &synthetic_challenge(seed))?;
            Ok(try_evaluate(&view, policy)?)
        }).collect::<Result<Vec<_>, SuiteError>>()?;
        Ok(ScenarioOutcome { label: scenario.to_string(), reports })
    }).collect()
}

/// Markdown tables: outcome counts and median statistics per scenario
/// (`n` counts evaluations: seeds, or captures × challenges).
pub fn markdown(outcomes: &[ScenarioOutcome]) -> String {
    let mut s = String::from("| scenario | n | PASS | not PASS | quality false | geometry false | rgb_depth false | temporal false | motion false | sole failing predicate (counts) |\n|---|---|---|---|---|---|---|---|---|---|\n");
    for o in outcomes {
        let n = o.reports.len();
        let f = o.failures();
        let sole: Vec<String> = o.sole_failures().iter().zip(PREDICATES).filter(|(c, _)| **c > 0).map(|(c, p)| format!("{p} {c}")).collect();
        s.push_str(&format!("| {} | {n} | {} | {} | {} | {} | {} | {} | {} | {} |\n", o.label, o.passes(), n - o.passes(),
            f[0], f[1], f[2], f[3], f[4], if sole.is_empty() { "-".to_owned() } else { sole.join(", ") }));
    }
    s.push_str("\nMedians:\n\n| scenario | V_all | V_c | max_all | max_c | tiles_nonplanar | tiles_nonplanar_c | n_strong | tiles_strong | N_disc | N_match | N_usable | N_occ | N_dok | N_inf | N_pok |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for o in outcomes {
        let m: [u16; 15] = [
            o.median(|s| s.v_all), o.median(|s| s.v_c), o.median(|s| s.max_all), o.median(|s| s.max_c),
            o.median(|s| s.tiles_nonplanar), o.median(|s| s.tiles_nonplanar_c), o.median(|s| s.n_strong), o.median(|s| s.tiles_strong),
            o.median(|s| s.n_disc), o.median(|s| s.n_match), o.median(|s| s.n_usable), o.median(|s| s.n_occ),
            o.median(|s| s.n_dok), o.median(|s| s.n_inf), o.median(|s| s.n_pok),
        ];
        let cells: Vec<String> = m.iter().map(u16::to_string).collect();
        s.push_str(&format!("| {} | {} |\n", o.label, cells.join(" | ")));
    }
    s
}
