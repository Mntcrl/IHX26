//! geometry-tool: synthetic captures, reference evaluation, GeometryView
//! export, policy fixtures, scenario report, IMU frame fit and calibration.
//! Never prints raw sensor values; outputs are reports, aggregates or files
//! written to an explicit path.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use galaxy_crypto::sampling::Challenge;
use galaxy_evidence::bundle::{self, EvidenceBundle};
use galaxy_evidence::{ingest, Evidence, ImageProfile, Salts};
use galaxy_geometry::synthetic::{self, suite, Scenario};
use galaxy_geometry::{build_view, diagnostics, fixtures, try_evaluate, GeometryView};
use galaxy_protocol::policy::Policy;

type Error = Box<dyn std::error::Error>;

#[derive(Parser)]
#[command(name = "geometry-tool", about = "galaxy-pc-v1m reference engine tools")]
struct Cli {
    /// Policy JSON used for evaluation.
    #[arg(long, global = true, default_value = "policies/galaxy-pc-v1m.json")]
    policy: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum Profile {
    Png,
    Jpeg,
}

impl From<Profile> for ImageProfile {
    fn from(p: Profile) -> Self {
        match p {
            Profile::Png => ImageProfile::PngV1,
            Profile::Jpeg => ImageProfile::JpegV1,
        }
    }
}

#[derive(clap::Args)]
struct ChallengeArgs {
    /// Capture id (64 hex chars).
    #[arg(long)]
    capture_id: String,
    /// Selection ledger index Lsel.
    #[arg(long)]
    lsel: u32,
    /// Hash of ledger Lsel (64 hex chars).
    #[arg(long)]
    lsel_hash: String,
    /// XRPL NetworkID (1 = testnet).
    #[arg(long, default_value_t = 1)]
    network_id: u32,
}

impl ChallengeArgs {
    fn derive(&self) -> Result<Challenge, Error> {
        Ok(Challenge::derive(self.network_id, &hash32(&self.capture_id)?, self.lsel, &hash32(&self.lsel_hash)?))
    }
}

#[derive(Subcommand)]
enum Command {
    /// Render a synthetic (simulated, not device) capture; writes the canonical
    /// image and its GeometryView (synthetic challenge, `expected` filled), and
    /// with `--gxev` a GXEV EvidenceBundle (fresh random salts, empty capture state).
    Synth {
        #[arg(value_parser = parse_scenario)]
        scenario: Scenario,
        #[arg(long)]
        seed: u64,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, value_enum, default_value = "png")]
        profile: Profile,
        #[arg(long)]
        gxev: bool,
    },
    /// Evaluate a GeometryView JSON (checking its `expected` if present) or a
    /// GXRW capture (requires the challenge arguments).
    Eval {
        input: PathBuf,
        #[command(flatten)]
        challenge: Option<ChallengeArgs>,
        #[arg(long, value_enum, default_value = "png")]
        profile: Profile,
    },
    /// Build the GeometryView of a GXRW capture for a ledger challenge.
    View {
        gxrw: PathBuf,
        #[command(flatten)]
        challenge: ChallengeArgs,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, value_enum, default_value = "png")]
        profile: Profile,
    },
    /// Fit the device→camera signed permutation m_cd from GXRW captures.
    ImuFrame {
        #[arg(required = true)]
        gxrw: Vec<PathBuf>,
    },
    /// Aggregate statistics over real captures: <dir>/<category>/*.gxrw,
    /// each evaluated under `--challenges` diagnostic (synthetic) challenges.
    Calibrate {
        dir: PathBuf,
        #[arg(long, default_value_t = 8)]
        challenges: u64,
    },
    /// Regenerate test-vectors/policy (GeometryView fixtures + README).
    Fixtures {
        #[arg(long, default_value = "test-vectors/policy")]
        out: PathBuf,
    },
    /// Run the synthetic scenario suite and print markdown tables.
    Report {
        #[arg(long, default_value_t = 20)]
        seeds: u64,
        #[arg(long, value_enum, default_value = "png")]
        profile: Profile,
    },
}

fn parse_scenario(s: &str) -> Result<Scenario, String> {
    s.parse().map_err(|_| format!("unknown scenario; one of: {}",
        Scenario::ALL.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")))
}

fn hash32(s: &str) -> Result<[u8; 32], Error> {
    let bytes = hex::decode(s)?;
    bytes.try_into().map_err(|_| "expected 32 bytes (64 hex chars)".into())
}

fn ingest_gxrw(path: &Path, profile: Profile) -> Result<Evidence, Error> {
    Ok(ingest::from_gxrw(&std::fs::read(path)?, profile.into())?)
}

fn is_gxrw(path: &Path) -> Result<bool, Error> {
    use std::io::Read;
    let mut magic = [0; 4];
    let n = std::fs::File::open(path)?.read(&mut magic)?;
    Ok(n == 4 && &magic == b"GXRW")
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

fn print_json(value: &impl serde::Serialize) -> Result<(), Error> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn run(cli: Cli) -> Result<ExitCode, Error> {
    let policy = Policy::load(&cli.policy)?;
    policy.validate()?;
    match cli.command {
        Command::Synth { scenario, seed, out, profile, gxev } => {
            let evidence = synthetic::synthesize_with_profile(scenario, seed, &policy, profile.into())?;
            let view = build_view(&evidence, &synthetic::synthetic_challenge(seed))?.with_expected(&policy);
            let stem = format!("synthetic-{scenario}-{seed}");
            let ext = match profile { Profile::Png => "png", Profile::Jpeg => "jpg" };
            write(&out.join(format!("{stem}.{ext}")), &evidence.image.encoded)?;
            write(&out.join(format!("{stem}.view.json")), &fixtures::to_json(&view))?;
            if gxev {
                let bundle = EvidenceBundle { evidence, salts: Salts::random()?, capture_state: Vec::new() };
                write(&out.join(format!("{stem}.gxev")), &bundle::encode(&bundle)?)?;
            }
            print_json(&view.expected)?;
        }
        Command::Eval { input, challenge, profile } => {
            let view = if is_gxrw(&input)? {
                let challenge = challenge.ok_or("GXRW input requires --capture-id, --lsel and --lsel-hash")?;
                build_view(&ingest_gxrw(&input, profile)?, &challenge.derive()?)?
            } else {
                serde_json::from_slice::<GeometryView>(&std::fs::read(&input)?)?
            };
            let report = try_evaluate(&view, &policy)?;
            print_json(&report)?;
            if view.expected.is_some_and(|e| e != report) {
                eprintln!("computed report differs from the view's `expected`");
                return Ok(ExitCode::FAILURE);
            }
        }
        Command::View { gxrw, challenge, out, profile } => {
            let view = build_view(&ingest_gxrw(&gxrw, profile)?, &challenge.derive()?)?.with_expected(&policy);
            write(&out, &fixtures::to_json(&view))?;
            print_json(&view.expected)?;
        }
        Command::ImuFrame { gxrw } => {
            // Only camera poses and IMU enter the fit; the sampled blocks of
            // this fixed diagnostic challenge are irrelevant to it.
            let challenge = synthetic::synthetic_challenge(0);
            let views = gxrw.iter().map(|p| Ok(build_view(&ingest_gxrw(p, Profile::Png)?, &challenge)?))
                .collect::<Result<Vec<_>, Error>>()?;
            let fit = diagnostics::fit_imu_frame(&views, &policy)
                .ok_or("no capture passed the view-domain and IMU-window checks")?;
            print_json(&fit)?;
        }
        Command::Calibrate { dir, challenges } => {
            let mut categories: Vec<PathBuf> = std::fs::read_dir(&dir)?.map(|e| Ok(e?.path()))
                .collect::<Result<Vec<_>, Error>>()?.into_iter().filter(|p| p.is_dir()).collect();
            categories.sort();
            let mut outcomes = Vec::new();
            for category in categories {
                let mut files: Vec<PathBuf> = std::fs::read_dir(&category)?.map(|e| Ok(e?.path()))
                    .collect::<Result<Vec<_>, Error>>()?.into_iter()
                    .filter(|p| p.extension().is_some_and(|x| x == "gxrw")).collect();
                files.sort();
                let mut reports = Vec::new();
                for file in &files {
                    let evidence = ingest_gxrw(file, Profile::Png)?;
                    for k in 0..challenges {
                        reports.push(try_evaluate(&build_view(&evidence, &synthetic::synthetic_challenge(k))?, &policy)?);
                    }
                }
                let name = category.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                outcomes.push(suite::ScenarioOutcome { label: format!("{name} ({} captures)", files.len()), reports });
            }
            print!("{}", suite::markdown(&outcomes));
        }
        Command::Fixtures { out } => {
            for (name, bytes) in fixtures::render_files(&policy)? {
                write(&out.join(&name), &bytes)?;
                println!("{}", out.join(&name).display());
            }
        }
        Command::Report { seeds, profile } => {
            let outcomes = suite::run(&policy, Scenario::ALL, 0..seeds, profile.into())?;
            print!("{}", suite::markdown(&outcomes));
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("geometry-tool: {e}");
            ExitCode::FAILURE
        }
    }
}
