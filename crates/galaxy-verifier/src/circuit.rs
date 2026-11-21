use crate::{pipeline::Stop, LedgerSource, VerifyContext};
use galaxy_crypto::groth16::VerifyingKey;
use galaxy_protocol::{
    ids,
    network::AcceptedCircuit,
    policy::Policy,
    wire::{CircuitBundle, CircuitManifest},
    Hash32,
};
use galaxy_xrpl::{ledger::ServerInfo, registry::{fetch_bundle, fetch_bundle_from_tx, Manifest}, Error as XrplError};
use serde::Serialize;
use serde_json::Value;

/// Displayable material for a circuit trusted by the selected config.
#[derive(Clone, Debug, Serialize)]
pub struct CircuitInfo {
    pub circuit_id: String,
    pub vk_digest: String,
    pub policy_id: String,
    pub registration_tx: String,
    pub descriptor: Value,
    pub policy: Policy,
    pub source: &'static str,
}

pub(crate) struct Circuit {
    pub info: CircuitInfo,
    pub vk: VerifyingKey,
    pub chunk_range: Option<(u32, u32)>,
    pub ignored_chunks: usize,
}

pub async fn inspect_circuit<L: LedgerSource>(
    circuit_id: Hash32,
    ctx: &VerifyContext<'_, L>,
) -> Result<CircuitInfo, crate::Report> {
    let mut report = ctx.report("circuit-inspect");
    match resolve(ctx, &circuit_id, None, None).await {
        Ok(circuit) => Ok(circuit.info),
        Err(stop) => {
            stop.apply(&mut report);
            Err(report)
        }
    }
}

pub(crate) fn accepted<'a, L: LedgerSource>(
    ctx: &VerifyContext<'a, L>,
    circuit_id: &Hash32,
    policy_id: Option<&Hash32>,
) -> Result<&'a AcceptedCircuit, Stop> {
    let entry = ctx.config.accepted_circuits.iter().find(|entry| &entry.circuit_id == circuit_id)
        .ok_or_else(|| Stop::unsupported("circuit.accepted", "Circuit is not on the configured allowlist"))?;
    if policy_id.is_some_and(|id| id != &entry.policy_id) {
        return Err(Stop::invalid("circuit.accepted", "CAPTURE policy differs from the configured policy for this circuit"));
    }
    Ok(entry)
}

pub(crate) async fn resolve<L: LedgerSource>(
    ctx: &VerifyContext<'_, L>,
    circuit_id: &Hash32,
    policy_id: Option<&Hash32>,
    server_info: Option<&ServerInfo>,
) -> Result<Circuit, Stop> {
    let entry = accepted(ctx, circuit_id, policy_id)?;
    let network = ctx.config.network_id.as_u32();
    if network == 0 && !ctx.config.allow_mainnet {
        return Err(Stop::unsupported("ledger.network", "Mainnet is not enabled in the network config"));
    }
    let check = "circuit.registration";
    let (bundle, manifest, source, ignored_chunks) = if let Some(bytes) = ctx.local_circuit {
        (CircuitBundle::decode(bytes).map_err(|err| Stop::invalid(check, err.to_string()))?, None, "local-digest-pinned", 0)
    } else {
        let hash = &entry.registration_tx;
        let fetched = async {
            if let Some(info) = server_info {
                let tx = ctx.ledger.tx(hash).await
                    .map_err(|_| XrplError::HistoryUnavailable("registry manifest transaction unavailable"))?
                    .ok_or(XrplError::HistoryUnavailable("registry manifest not found"))?;
                fetch_bundle_from_tx(ctx.ledger, hash, &tx, info, network, ctx.config.allow_mainnet).await
            } else {
                fetch_bundle(ctx.ledger, hash, network, ctx.config.allow_mainnet).await
            }
        }.await.map_err(|err| Stop::from_rpc(check, &err, true))?;
        let Manifest::Circuit(manifest) = fetched.manifest else {
            return Err(Stop::invalid(check, "Configured registration is not a CIRCUIT manifest"));
        };
        let bundle = CircuitBundle::decode(&fetched.bytes).map_err(|err| Stop::invalid(check, err.to_string()))?;
        (bundle, Some(manifest), "xrpl-digest-pinned", fetched.ignored_records)
    };
    let mut circuit = check_bundle(entry, bundle, manifest, source)?;
    circuit.ignored_chunks = ignored_chunks;
    Ok(circuit)
}

fn check_bundle(
    entry: &AcceptedCircuit,
    bundle: CircuitBundle,
    manifest: Option<CircuitManifest>,
    source: &'static str,
) -> Result<Circuit, Stop> {
    let check = "circuit.registration";
    let malformed = |reason| Stop::invalid(check, reason);
    if manifest.as_ref().is_some_and(|m| {
        m.circuit_id != entry.circuit_id || m.policy_id != entry.policy_id || m.vk_digest != entry.vk_digest
    }) || ids::circuit_id(&bundle.descriptor_json).ok() != Some(entry.circuit_id)
        || ids::policy_id(&bundle.policy_json).ok() != Some(entry.policy_id)
        || ids::vk_digest(&bundle.vk_bytes) != entry.vk_digest
    {
        return Err(malformed("Descriptor, policy or verifying-key digest differs from the configured circuit"));
    }
    let descriptor: Value = serde_json::from_slice(&bundle.descriptor_json)
        .map_err(|_| malformed("Invalid circuit descriptor JSON"))?;
    let digest_eq = |key: &str, expected: &Hash32| {
        let mut hash = [0; 32];
        descriptor[key].as_str().is_some_and(|s| hex::decode_to_slice(s, &mut hash).is_ok() && &hash == expected)
    };
    if descriptor["public_layout"] != "galaxy-pc-v1-public-22"
        || descriptor["n_public"] != 22
        || !digest_eq("policy_id", &entry.policy_id)
        || !digest_eq("vk_digest", &entry.vk_digest)
        || descriptor["ceremony"].as_str().is_none_or(str::is_empty)
    {
        return Err(malformed("Circuit descriptor does not bind the configured policy, VK and v1 public input layout"));
    }
    let policy: Policy = serde_json::from_slice(&bundle.policy_json)
        .map_err(|_| malformed("Invalid Galaxy v1 policy JSON"))?;
    policy.validate().map_err(|err| Stop::invalid(check, err.to_string()))?;
    // The v1 VK header contains the public input count (and is strictly decoded below).
    if bundle.vk_bytes.get(2..6) != Some(&22u32.to_be_bytes()) {
        return Err(malformed("The verifying key does not declare 22 public inputs"));
    }
    let vk = VerifyingKey::from_bytes(&bundle.vk_bytes)
        .map_err(|err| Stop::invalid(check, err.to_string()))?;
    let info = CircuitInfo {
        circuit_id: hex::encode(entry.circuit_id), vk_digest: hex::encode(entry.vk_digest),
        policy_id: hex::encode(entry.policy_id), registration_tx: hex::encode_upper(entry.registration_tx),
        descriptor, policy, source,
    };
    Ok(Circuit {
        info, vk, chunk_range: manifest.map(|m| (m.first_chunk_ledger, m.last_chunk_ledger)),
        ignored_chunks: 0,
    })
}
