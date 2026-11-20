//! DEVELOPMENT desktop prover CLI (labelled dev path; production proofs are
//! generated on the iPhone). Used for benchmarks and snarkjs cross-checks.
//!
//! ```text
//! galaxy-prover prove   --graph G --zkey Z --input I [--out DIR] [--msm-chunk N] [--repeat N]
//!                       [--graph-sha256 HEX --zkey-sha256 HEX]   # pinned open (descriptor digests)
//! galaxy-prover witness --graph G --input I [--repeat N]
//! galaxy-prover verify  --vk VK.bin --public public.json --proof proof.json|proof.bin
//! galaxy-prover vk      --zkey Z --out VK.bin
//! galaxy-prover dev-setup --r1cs C.r1cs --ptau P.ptau --ptau-sha256 HEX --out C.zkey
//!                         # DEVELOPMENT setup: verified phase-1 ptau + single-party phase 2
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use galaxy_prover::{
    codec, ArtifactDigests, Phase, Progress, ProveControl, Prover, ProverConfig, ProverError,
};

struct Args(HashMap<String, String>);

impl Args {
    fn parse(rest: &[String]) -> Result<Self, String> {
        let mut m = HashMap::new();
        let mut it = rest.iter();
        while let Some(k) = it.next() {
            let key = k
                .strip_prefix("--")
                .ok_or_else(|| format!("unexpected argument {k}"))?;
            let v = it.next().ok_or_else(|| format!("missing value for {k}"))?;
            m.insert(key.to_owned(), v.clone());
        }
        Ok(Self(m))
    }
    fn path(&self, k: &str) -> Result<PathBuf, String> {
        self.0
            .get(k)
            .map(PathBuf::from)
            .ok_or_else(|| format!("--{k} is required"))
    }
    fn num(&self, k: &str, default: usize) -> Result<usize, String> {
        self.0
            .get(k)
            .map_or(Ok(default), |v| v.parse().map_err(|_| format!("bad --{k}")))
    }
    /// Optional 32-byte digest given as 64 hex digits.
    fn sha256(&self, k: &str) -> Result<Option<[u8; 32]>, String> {
        let Some(hex) = self.0.get(k) else {
            return Ok(None);
        };
        let bad = || format!("--{k} must be 64 hex digits");
        if hex.len() != 64 || !hex.is_ascii() {
            return Err(bad());
        }
        let mut d = [0u8; 32];
        for (i, b) in d.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| bad())?;
        }
        Ok(Some(d))
    }
}

struct Stderr;

impl Progress for Stderr {
    fn phase_started(&self, _: Phase) {}
    fn phase_finished(&self, phase: Phase, seconds: f64) {
        eprintln!("  {:<12} {:>9.3} s", phase.name(), seconds);
    }
}

fn read(p: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(p).map_err(|e| format!("read {}: {e}", p.display()))
}

fn write(p: &Path, b: &[u8]) -> Result<(), String> {
    std::fs::write(p, b).map_err(|e| format!("write {}: {e}", p.display()))
}

fn err(e: ProverError) -> String {
    e.to_string()
}

fn run() -> Result<(), String> {
    let argv: Vec<String> = std::env::args().collect();
    let cmd = argv.get(1).cloned().unwrap_or_default();
    let a = Args::parse(argv.get(2..).unwrap_or(&[]))?;
    match cmd.as_str() {
        "prove" => {
            let cfg = ProverConfig {
                msm_chunk: a.num("msm-chunk", ProverConfig::default().msm_chunk)?,
            };
            let t0 = Instant::now();
            let (graph, zkey) = (a.path("graph")?, a.path("zkey")?);
            let prover = match (a.sha256("graph-sha256")?, a.sha256("zkey-sha256")?) {
                (Some(witness_graph_sha256), Some(zkey_sha256)) => {
                    let digests = ArtifactDigests {
                        witness_graph_sha256,
                        zkey_sha256,
                    };
                    Prover::open_pinned(&graph, &zkey, &digests, cfg).map_err(err)?
                }
                (None, None) => Prover::open_with_config(&graph, &zkey, cfg).map_err(err)?,
                _ => return Err("--graph-sha256 and --zkey-sha256 go together".into()),
            };
            eprintln!(
                "[DEV desktop prover] artifacts opened in {:.3} s",
                t0.elapsed().as_secs_f64()
            );
            let input = String::from_utf8(read(&a.path("input")?)?).map_err(|e| e.to_string())?;
            let ctl = ProveControl {
                progress: Some(&Stderr),
                cancel: None,
            };
            let mut last = None;
            for i in 0..a.num("repeat", 1)? {
                eprintln!("run {i}:");
                let t = Instant::now();
                let out = prover.prove_ark(&input, &ctl).map_err(err)?;
                eprintln!(
                    "  witness {:.3} s, prove {:.3} s, total {:.3} s (wall {:.3} s)",
                    out.2.witness(),
                    out.2.prove(),
                    out.2.total(),
                    t.elapsed().as_secs_f64()
                );
                last = Some(out);
            }
            let (proof, public, timings) = last.ok_or("--repeat must be >= 1")?;
            if let Some(dir) = a.0.get("out").map(PathBuf::from) {
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                write(
                    &dir.join("proof.bin"),
                    &codec::encode_proof(&proof).map_err(err)?,
                )?;
                write(
                    &dir.join("proof.json"),
                    codec::proof_to_snarkjs_json(&proof)
                        .map_err(err)?
                        .as_bytes(),
                )?;
                write(
                    &dir.join("public.json"),
                    codec::public_to_snarkjs_json(&public).as_bytes(),
                )?;
                write(&dir.join("vk.bin"), &prover.vk_bytes().map_err(err)?)?;
                let t: serde_json::Map<String, serde_json::Value> = Phase::ALL
                    .iter()
                    .map(|p| (p.name().to_owned(), timings.get(*p).into()))
                    .collect();
                write(
                    &dir.join("timings.json"),
                    serde_json::to_string_pretty(&t).expect("json").as_bytes(),
                )?;
            }
            Ok(())
        }
        "witness" => {
            let graph =
                galaxy_prover::witness::WitnessGraph::open(&a.path("graph")?).map_err(err)?;
            let input = String::from_utf8(read(&a.path("input")?)?).map_err(|e| e.to_string())?;
            for _ in 0..a.num("repeat", 1)? {
                let t = Instant::now();
                let w = graph.calculate(&input).map_err(err)?;
                eprintln!(
                    "witness: {} wires in {:.3} s",
                    w.len(),
                    t.elapsed().as_secs_f64()
                );
            }
            Ok(())
        }
        "verify" => {
            let vk = read(&a.path("vk")?)?;
            let public = codec::public_from_snarkjs_json(
                &String::from_utf8(read(&a.path("public")?)?).map_err(|e| e.to_string())?,
            )
            .map_err(err)?;
            let pp = a.path("proof")?;
            let raw = read(&pp)?;
            let proof = if pp.extension().is_some_and(|e| e == "json") {
                codec::proof_from_snarkjs_json(&String::from_utf8(raw).map_err(|e| e.to_string())?)
                    .map_err(err)?
            } else {
                codec::decode_proof(&raw).map_err(err)?
            };
            let ok =
                galaxy_prover::verify_ark(&codec::decode_vk(&vk).map_err(err)?, &public, &proof)
                    .map_err(err)?;
            println!("{}", if ok { "OK" } else { "INVALID" });
            if ok {
                Ok(())
            } else {
                Err("proof does not verify".into())
            }
        }
        "vk" => {
            let zkey = galaxy_prover::zkey::Zkey::open(&a.path("zkey")?).map_err(err)?;
            write(
                &a.path("out")?,
                &codec::encode_vk(zkey.verifying_key()).map_err(err)?,
            )
        }
        "dev-setup" => {
            let t = Instant::now();
            let digest = a
                .sha256("ptau-sha256")?
                .ok_or("--ptau-sha256 is required")?;
            let info = galaxy_prover::dev_setup::dev_setup(
                &a.path("r1cs")?,
                &a.path("ptau")?,
                &digest,
                &a.path("out")?,
            )
            .map_err(err)?;
            eprintln!(
                "[DEVELOPMENT setup: ptau phase 1 + single-party phase 2, never for accepted circuits] {info:?} in {:.1} s",
                t.elapsed().as_secs_f64()
            );
            Ok(())
        }
        _ => Err("usage: galaxy-prover <prove|witness|verify|vk|dev-setup> --key value ...".into()),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
