use std::path::PathBuf;
use std::time::Instant;

use clap::Args;
use galaxy_protocol::network::NetworkConfig;
use galaxy_verifier::{
    recorded::{MAX_RECORDING_BYTES, Recorder},
    verify_photo, State, Status, VerifyContext,
};
use galaxy_xrpl::ledger::RpcLedger;
use serde_json::json;

use crate::{error, output, rpc, storage, Result};

#[derive(Args)]
pub(crate) struct RecordArgs {
    /// Development image with embedded XMP CAPTURE and PROOF references.
    image: PathBuf,
    /// Output a replayable array of public, read-only XRPL RPC exchanges.
    #[arg(long)]
    output: PathBuf,
}

pub(crate) async fn run(config: &NetworkConfig, args: RecordArgs) -> Result<()> {
    let image = storage::read(&args.image, galaxy_image::MAX_IMAGE_BYTES)?;
    let client = rpc(config)?;
    let mut endpoints = client.pinned_endpoints().peekable();
    let started = Instant::now();
    let (report, recorder) = loop {
        let (_, endpoint) = endpoints.next().ok_or_else(|| error("no Testnet RPC endpoint"))?;
        let recorder = Recorder::new(endpoint);
        let ledger = RpcLedger::new(&recorder);
        let ctx = VerifyContext::new(&ledger, config);
        let report = verify_photo(&image, None, &ctx).await;
        if endpoints.peek().is_none() || !report.needs_rpc_retry() {
            break (report, recorder);
        }
    };
    let verify_ms = started.elapsed().as_millis();
    // Only persist a complete RPC session for a proven development image.
    if report.state != State::Unsupported
        || !report.summary.contains("development unattested credential")
        || ![
            "capture.photo_binding", "capture.rgb_binding", "proof.groth16",
            "registration.txn_chain", "capture.txn_chain", "history.complete",
        ].iter().all(|id| {
            report.checks.iter().any(|check| check.id == *id && check.status == Status::Pass)
        })
        || report.checks.iter().any(|check| match check.status {
            Status::Fail => check.id != "registration.attestation",
            Status::Unavailable | Status::Skipped => true,
            Status::Pass | Status::NotApplicable => false,
        })
    {
        return Err(error(&format!("cannot record incomplete development verification: {}: {}", report.state.as_str(), report.summary)));
    }
    let recording = recorder.recording();
    let bytes = recording.to_json();
    if bytes.len() > MAX_RECORDING_BYTES {
        return Err(error("development RPC recording exceeds replay limit"));
    }
    storage::write_atomic(&args.output, bytes.as_bytes())?;
    output(json!({"state": report.state, "verification_ms": verify_ms, "rpc_exchanges": recording.exchanges.len(), "recording_bytes": bytes.len()}));
    Ok(())
}
