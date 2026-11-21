//! Offline XRPL history built from real signed transactions (throwaway test
//! wallets, galaxy-xrpl signing), replayed through `RecordedLedgerSource`, and a
//! development Groth16 circuit with the v1 public input count (22). All key
//! material here is test-only and never accepted outside these tests.
#![allow(dead_code)]

use ark_bn254::{Bn254, Fr};
use ark_groth16::{Groth16, ProvingKey};
use ark_relations::gr1cs::{ConstraintSynthesizer, ConstraintSystemRef, SynthesisError};
use ark_std::rand::{rngs::StdRng, SeedableRng};
use ed25519_dalek::{Signer, SigningKey};
use galaxy_crypto::{field, groth16, public::public_inputs_v1, sampling};
use galaxy_image::ImageProfile;
use galaxy_protocol::{
    canonical_json, ids,
    network::{AcceptedCircuit, NetworkConfig},
    wire::{
        AttestationEnv, CaptureCore, CaptureMemo, ChunkMemo, CircuitBundle, CircuitManifest, DeviceBundle, DeviceKind,
        DeviceManifest, MemoKind, ProofMemo, RevokeMemo,
    },
    xmp::GalaxyXmp,
    AccountId, Hash32,
};
use galaxy_verifier::{recorded::Recording, LookupError, LookupResult, LookupSource, Refs, VerifyContext};
use galaxy_xrpl::{
    codec,
    keys::{classic_address, Wallet},
    ledger::{account_root_keylet, RpcLedger},
    transaction::{AccountSet, Memo},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

pub const NETWORK: u32 = 1;
pub const INCEPTION: u32 = 1000;
pub const CIRCUIT_CHUNKS: u32 = 1001;
pub const CIRCUIT_MANIFEST: u32 = 1002;
pub const REG_LEDGER: u32 = 1003;
pub const DEVICE_CHUNKS: u32 = 1004;
pub const DEVICE_MANIFEST: u32 = 1005;
pub const LPRE: u32 = 1010;
pub const L0: u32 = 1012;
pub const LSEL: u32 = L0 + 5;
pub const DEADLINE: u32 = LSEL + 300;
pub const PROOF_LEDGER: u32 = 1020;
pub const TIP: u32 = 1400;
pub const APP_ID: &str = "PPS57BF3X3.utcq.XRPL-Galaxy";

pub fn ledger_hash(index: u32) -> Hash32 {
    Sha256::new().chain_update(b"verifier-test-ledger").chain_update(index.to_be_bytes()).finalize().into()
}

fn ripple_close_time(index: u32) -> u32 {
    // 2026-11-20T00:00:00Z in Ripple epoch seconds, then 4 s per ledger.
    846_979_200 + index * 4
}

#[derive(Clone)]
pub struct Tx {
    pub hash: Hash32,
    pub blob: Vec<u8>,
    pub ledger: u32,
    pub index: u32,
    pub result: &'static str,
    pub validated: bool,
    pub account: AccountId,
    pub predecessor: Hash32,
}

/// A synthetic validated ledger history holding real signed transaction blobs.
#[derive(Clone)]
pub struct Chain {
    pub txs: Vec<Tx>,
    pub tip: u32,
    pub network_id: u32,
    pub complete: Option<String>,
    /// account_tx queries to answer: (account, min, max or the tip).
    pub queries: Vec<(AccountId, u32, Option<u32>)>,
    pub page_size: Option<usize>,
    pub overrides: Vec<(Hash32, Value)>,
    sequence: u32,
}

impl Chain {
    pub fn new() -> Self {
        Self {
            txs: Vec::new(), tip: TIP, network_id: NETWORK, complete: None, queries: Vec::new(),
            page_size: None, overrides: Vec::new(), sequence: 1,
        }
    }

    pub fn add(&mut self, wallet: &Wallet, kind: &str, data: &[u8], ledger: u32) -> Hash32 {
        self.add_with(wallet, kind, data, ledger, "tesSUCCESS")
    }

    pub fn add_with(&mut self, wallet: &Wallet, kind: &str, data: &[u8], ledger: u32, result: &'static str) -> Hash32 {
        let account = wallet.account_id();
        let predecessor = self.txs.iter().filter(|tx| {
            tx.account == account && tx.result == "tesSUCCESS" && tx.validated && tx.ledger <= ledger
        }).max_by_key(|tx| (tx.ledger, tx.index)).map_or([0; 32], |tx| tx.hash);
        let account_txn_id = (!matches!(MemoKind::from_memo_type(kind), Some(MemoKind::Circuit | MemoKind::CircuitChunk)))
            .then_some(predecessor);
        let tx = AccountSet {
            account, sequence: self.sequence, fee_drops: 12,
            last_ledger_sequence: ledger + 20, network_id: self.network_id.min(NETWORK),
            memo: Memo { kind, data }, account_txn_id,
        }
        .sign(wallet)
        .expect("sign");
        self.sequence += 1;
        let index = self.txs.iter().filter(|t| t.ledger == ledger).count() as u32;
        self.txs.push(Tx {
            hash: tx.hash, blob: tx.blob, ledger, index, result, validated: true, account, predecessor,
        });
        tx.hash
    }

    pub fn tx_mut(&mut self, hash: &Hash32) -> &mut Tx {
        self.txs.iter_mut().find(|t| &t.hash == hash).expect("known tx")
    }

    fn meta(tx: &Tx) -> String {
        hex::encode_upper(
            codec::encode(&json!({
                "AffectedNodes": [{"ModifiedNode": {
                    "LedgerEntryType": "AccountRoot", "LedgerIndex": hex::encode_upper(account_root_keylet(&tx.account)),
                    "FinalFields": {"Account": classic_address(&tx.account), "AccountTxnID": hex::encode_upper(tx.hash)},
                    "PreviousFields": {"AccountTxnID": hex::encode_upper(tx.predecessor)},
                }}], "TransactionIndex": tx.index, "TransactionResult": tx.result,
            }))
            .expect("meta"),
        )
    }

    fn validated(&self, tx: &Tx) -> bool {
        tx.validated && tx.ledger <= self.tip
    }

    fn header(index: u32) -> Value {
        let hash = hex::encode_upper(ledger_hash(index));
        json!({
            "ledger_index": index, "ledger_hash": hash, "validated": true,
            "ledger": {
                "closed": true, "ledger_hash": hash, "ledger_index": index,
                "parent_hash": hex::encode_upper(ledger_hash(index - 1)),
                "transaction_hash": hex::encode_upper([0u8; 32]),
                "close_time": ripple_close_time(index),
            },
        })
    }

    pub fn recording(&self) -> Recording {
        let mut rec = Recording::default();
        rec.push("server_info", json!({}), json!({"info": {
            "network_id": self.network_id,
            "complete_ledgers": self.complete.clone().unwrap_or_else(|| format!("{}-{}", INCEPTION - 10, self.tip)),
            "validated_ledger": {"seq": self.tip},
        }}));
        for (hash, result) in &self.overrides {
            rec.push("tx", json!({"transaction": hex::encode_upper(hash), "binary": true}), result.clone());
        }
        for tx in &self.txs {
            rec.push("tx", json!({"transaction": hex::encode_upper(tx.hash), "binary": true}), json!({
                "hash": hex::encode_upper(tx.hash), "tx_blob": hex::encode_upper(&tx.blob), "meta_blob": Self::meta(tx),
                "ledger_index": tx.ledger, "validated": self.validated(tx), "date": ripple_close_time(tx.ledger),
            }));
        }
        for index in INCEPTION - 10..=self.tip {
            rec.push("ledger", json!({"ledger_index": index, "transactions": false}), Self::header(index));
        }
        let mut queries = self.queries.clone();
        for &(account, from, to) in &self.queries {
            if from == DEVICE_MANIFEST && to.is_none() {
                let address = classic_address(&account);
                let anchor = self.txs.iter().filter(|tx| tx.account == account && tx.result == "tesSUCCESS" && self.validated(tx))
                    .max_by_key(|tx| (tx.ledger, tx.index)).map_or([0; 32], |tx| tx.hash);
                rec.push("account_info", json!({"account": address, "ledger_index": self.tip}), json!({
                    "validated": true, "ledger_index": self.tip, "account_data": {
                        "LedgerEntryType": "AccountRoot", "index": hex::encode_upper(account_root_keylet(&account)),
                        "Account": address, "AccountTxnID": hex::encode_upper(anchor),
                    },
                }));
                for tx in self.txs.iter().filter(|tx| tx.account == account && self.validated(tx) && tx.ledger >= from) {
                    queries.push((account, from, Some(tx.ledger)));
                    let select = tx.ledger + 5;
                    let through = (select + 300).min(self.tip);
                    if select < through { queries.push((account, select + 1, Some(through))); }
                }
            }
        }
        queries.sort_unstable();
        queries.dedup();
        for (account, min, max) in queries {
            let max = max.unwrap_or(self.tip);
            let mut entries: Vec<&Tx> = self.txs.iter()
                .filter(|t| t.account == account && (min..=max).contains(&t.ledger) && self.validated(t))
                .collect();
            entries.sort_by_key(|t| (t.ledger, t.index));
            let entries: Vec<Value> = entries.iter().map(|tx| json!({
                "tx_blob": hex::encode_upper(&tx.blob), "meta_blob": Self::meta(tx), "ledger_index": tx.ledger, "validated": true,
            })).collect();
            let size = self.page_size.unwrap_or(usize::MAX).max(1);
            let pages: Vec<&[Value]> = if entries.is_empty() { vec![&[]] } else { entries.chunks(size).collect() };
            let address = classic_address(&account);
            for (page, items) in pages.iter().enumerate() {
                let mut params = json!({
                    "account": address, "ledger_index_min": min, "ledger_index_max": max,
                    "binary": true, "forward": true, "limit": 200,
                });
                if page > 0 {
                    params["marker"] = json!({"page": page});
                }
                let mut result = json!({
                    "account": address, "ledger_index_min": min, "ledger_index_max": max,
                    "validated": true, "transactions": items,
                });
                if page + 1 < pages.len() {
                    result["marker"] = json!({"page": page + 1});
                }
                rec.push("account_tx", params, result);
            }
        }
        rec
    }

    pub fn ledger(&self) -> RpcLedger<Recording> {
        self.recording().into_ledger()
    }

    /// Record that a transaction id does not exist (`txnNotFound`).
    pub fn missing(&mut self, hash: Hash32) {
        self.overrides.push((hash, json!({"error": "txnNotFound"})));
    }
}

/// Development circuit with the v1 public layout: x_i · x_i = s_i for the 22 public inputs.
#[derive(Clone)]
struct Squares([Fr; 22]);

impl ConstraintSynthesizer<Fr> for Squares {
    fn generate_constraints(self, cs: ConstraintSystemRef<Fr>) -> Result<(), SynthesisError> {
        for x in self.0 {
            let input = cs.new_input_variable(|| Ok(x))?;
            let square = cs.new_witness_variable(|| Ok(x * x))?;
            cs.enforce_r1cs_constraint(|| input.into(), || input.into(), || square.into())?;
        }
        Ok(())
    }
}

static PROVING_KEY: LazyLock<ProvingKey<Bn254>> = LazyLock::new(|| {
    let mut rng = StdRng::seed_from_u64(0x6761_6c61_7879);
    Groth16::<Bn254>::generate_random_parameters_with_reduction(Squares([Fr::from(0u8); 22]), &mut rng).expect("setup")
});

pub fn prove(public: &[Fr; 22]) -> [u8; 256] {
    let mut rng = StdRng::seed_from_u64(7);
    let proof = Groth16::<Bn254>::create_random_proof_with_reduction(Squares(*public), &PROVING_KEY, &mut rng)
        .expect("prove");
    groth16::Proof::from_ark(proof).expect("proof").to_bytes()
}

pub fn vk_bytes() -> Vec<u8> {
    groth16::VerifyingKey::from_ark(PROVING_KEY.vk.clone()).expect("vk").to_bytes()
}

pub fn circuit_bundle() -> (CircuitBundle, AcceptedCircuitIds) {
    // Snapshot policy; calibration of the active deployment must not change
    // the committed recording, transaction hashes, or replay expectations.
    let policy_json = include_bytes!("../../../../test-vectors/verifier/dev-unattested/policy.json").to_vec();
    let policy_id = ids::policy_id(&policy_json).expect("canonical fixture policy");
    let vk = vk_bytes();
    let vk_digest = ids::vk_digest(&vk);
    let descriptor = canonical_json::to_vec(&json!({
        "ceremony": "development-single-party", "circom_version": "none", "n_constraints": 22, "n_public": 22,
        "name": "verifier-test-squares", "policy_id": hex::encode(policy_id), "profile": "galaxy-pc-v1m",
        "public_layout": "galaxy-pc-v1-public-22", "version": 1, "vk_digest": hex::encode(vk_digest),
    }))
    .expect("descriptor");
    let circuit_id = ids::circuit_id(&descriptor).expect("circuit id");
    (CircuitBundle { descriptor_json: descriptor, vk_bytes: vk, policy_json }, AcceptedCircuitIds { circuit_id, policy_id, vk_digest })
}

#[derive(Clone, Copy)]
pub struct AcceptedCircuitIds {
    pub circuit_id: Hash32,
    pub policy_id: Hash32,
    pub vk_digest: Hash32,
}

pub fn raster(seed: u8) -> Vec<u8> {
    (0..galaxy_image::RGB_LEN).map(|i| ((i * 31 + (i / 3072) * 7 + usize::from(seed) * 13) % 251) as u8).collect()
}

pub fn png(rgb: &[u8]) -> Vec<u8> {
    galaxy_image::encode(ImageProfile::PngV1, rgb).expect("png")
}

pub struct CaptureInfo {
    pub hash: Hash32,
    pub core: CaptureCore,
    pub capture_id: Hash32,
    pub l0: u32,
}

/// Circuit registration, a dev-unattested DEVICE registration and helpers to publish captures and proofs.
pub struct World {
    pub chain: Chain,
    pub config: NetworkConfig,
    pub circuit: CircuitBundle,
    pub ids: AcceptedCircuitIds,
    pub circuit_tx: Hash32,
    pub device: Wallet,
    pub dev_key: SigningKey,
    pub registration_tx: Hash32,
}

pub fn publish_bundle(chain: &mut Chain, wallet: &Wallet, kind: MemoKind, bytes: &[u8], ledger: u32) -> (Hash32, u16) {
    let id = ids::bundle_id(bytes);
    let total = bytes.len().div_ceil(800) as u16;
    for (index, payload) in bytes.chunks(800).enumerate() {
        let memo = ChunkMemo { bundle_id: id, index: index as u16, total, payload: payload.to_vec() }.encode();
        chain.add(wallet, kind.as_str(), &memo, ledger);
    }
    (id, total)
}

impl World {
    pub fn new() -> Self {
        Self::with_circuit(circuit_bundle().0)
    }

    /// A world whose accepted circuit is `circuit` (e.g. the exported main_v1m bundle).
    pub fn with_circuit(circuit: CircuitBundle) -> Self {
        let ids = AcceptedCircuitIds {
            circuit_id: ids::circuit_id(&circuit.descriptor_json).expect("circuit id"),
            policy_id: ids::policy_id(&circuit.policy_json).expect("policy id"),
            vk_digest: ids::vk_digest(&circuit.vk_bytes),
        };
        let mut chain = Chain::new();
        let publisher = Wallet::from_entropy(&[0x11; 16]);
        let device = Wallet::from_entropy(&[0x22; 16]);
        let bytes = circuit.encode();
        let (bundle_id, total) = publish_bundle(&mut chain, &publisher, MemoKind::CircuitChunk, &bytes, CIRCUIT_CHUNKS);
        let manifest = CircuitManifest {
            network: NETWORK, bundle_id, bundle_len: bytes.len() as u32, chunk_total: total,
            first_chunk_ledger: CIRCUIT_CHUNKS, last_chunk_ledger: CIRCUIT_CHUNKS,
            circuit_id: ids.circuit_id, policy_id: ids.policy_id, vk_digest: ids.vk_digest,
        };
        let circuit_tx = chain.add(&publisher, MemoKind::Circuit.as_str(), &manifest.encode(), CIRCUIT_MANIFEST);
        chain.queries.push((publisher.account_id(), CIRCUIT_CHUNKS, Some(CIRCUIT_CHUNKS)));

        let dev_key = SigningKey::from_bytes(&[0x33; 32]);
        let public = dev_key.verifying_key().to_bytes();
        let bundle = DeviceBundle {
            kind: DeviceKind::DevUnattested, env: AttestationEnv::NotApplicable, app_id: APP_ID.into(),
            key_id: ids::dev_key_id(&public), network: NETWORK, reg_ledger_index: REG_LEDGER,
            reg_ledger_hash: ledger_hash(REG_LEDGER), reg_nonce: [0x44; 32], account_id: device.account_id(),
            attestation: public.to_vec(),
        };
        let registration_tx = register_device(&mut chain, &device, &bundle);

        let text = include_str!("../../../../deployments/networks/xrpl-testnet.toml");
        let mut config = NetworkConfig::from_toml(text).expect("config");
        config.inception_ledger = INCEPTION;
        config.accepted_circuits = vec![AcceptedCircuit {
            circuit_id: ids.circuit_id, vk_digest: ids.vk_digest, policy_id: ids.policy_id, registration_tx: circuit_tx,
        }];
        Self { chain, config, circuit, ids, circuit_tx, device, dev_key, registration_tx }
    }

    pub fn ctx<'a>(&'a self, ledger: &'a RpcLedger<Recording>) -> VerifyContext<'a, RpcLedger<Recording>> {
        let mut ctx = VerifyContext::new(ledger, &self.config);
        ctx.rpc_endpoints = vec!["recording".into()];
        ctx
    }

    pub fn core(&self, rgb: &[u8], nonce: u8) -> CaptureCore {
        let mut evidence_root = [0u8; 32];
        evidence_root[31] = 7;
        evidence_root[0] = nonce & 0x0f;
        self.core_with(rgb, nonce, evidence_root)
    }

    pub fn core_with(&self, rgb: &[u8], nonce: u8, evidence_root: Hash32) -> CaptureCore {
        CaptureCore {
            network: NETWORK, registration_tx: self.registration_tx, preledger_hash: ledger_hash(LPRE),
            nonce: [nonce; 32], preledger_index: LPRE, claimed_time_ms: 1_795_117_441_000, width: 1024, height: 768,
            flags: 0, photo_id: galaxy_image::photo_id(rgb).expect("photo id"),
            rgb_root: field::fr_to_be(&galaxy_image::rgb_root_from_raster(rgb).expect("rgb root")),
            evidence_root,
            policy_id: self.ids.policy_id, circuit_id: self.ids.circuit_id, account_id: self.device.account_id(),
        }
    }

    pub fn capture_memo(&self, core: &CaptureCore) -> Vec<u8> {
        let core299 = core.encode();
        let signature = self.dev_key.sign(&ids::dev_assertion_message(&core299));
        let mut assertion = vec![0xf0];
        assertion.extend_from_slice(&signature.to_bytes());
        CaptureMemo { core: core.clone(), assertion }.encode()
    }

    pub fn publish_capture(&mut self, core: CaptureCore, l0: u32) -> CaptureInfo {
        let memo = self.capture_memo(&core);
        let hash = self.chain.add(&self.device, MemoKind::Capture.as_str(), &memo, l0);
        CaptureInfo { hash, capture_id: ids::capture_id(&core.encode()), core, l0 }
    }

    pub fn public_inputs(&self, capture: &CaptureInfo) -> [Fr; 22] {
        let lsel = capture.l0 + 5;
        let seed = sampling::seed(NETWORK, &capture.capture_id, lsel, &ledger_hash(lsel));
        let tiles = sampling::select_tiles(&seed);
        let rgb = field::fr_from_be_canonical(&capture.core.rgb_root).expect("rgb");
        let evidence = field::fr_from_be_canonical(&capture.core.evidence_root).expect("evidence");
        public_inputs_v1(&rgb, &evidence, &capture.capture_id, &seed, &tiles)
    }

    pub fn add_device_tx(&mut self, kind: MemoKind, data: &[u8], ledger: u32) -> Hash32 {
        self.chain.add(&self.device, kind.as_str(), data, ledger)
    }

    pub fn proof_memo(&self, capture: &CaptureInfo, proof: [u8; 256]) -> Vec<u8> {
        let lsel = capture.l0 + 5;
        ProofMemo {
            network: NETWORK, manifest_tx: capture.hash, selection_ledger_index: lsel,
            selection_ledger_hash: ledger_hash(lsel), circuit_id: self.ids.circuit_id, proof,
        }
        .encode()
    }

    pub fn publish_proof(&mut self, capture: &CaptureInfo, ledger: u32) -> Hash32 {
        let memo = self.proof_memo(capture, prove(&self.public_inputs(capture)));
        self.chain.add(&self.device, MemoKind::Proof.as_str(), &memo, ledger)
    }

    pub fn revoke(&mut self, ledger: u32, effective_ledger: u32) -> Hash32 {
        let memo = RevokeMemo { network: NETWORK, registration_tx: self.registration_tx, reason: 1, effective_ledger }.encode();
        self.chain.add(&self.device, MemoKind::Revoke.as_str(), &memo, ledger)
    }

    pub fn xmp(&self, capture: &CaptureInfo, proof_tx: Hash32) -> GalaxyXmp {
        GalaxyXmp {
            protocol: "galaxy/v1".into(), network: "xrpl-testnet".into(), image_profile: "png-v1".into(),
            photo_id: capture.core.photo_id, capture_id: capture.capture_id, manifest_tx: capture.hash, proof_tx,
        }
    }
}

/// Publish chunks, then the DEVICE manifest two ledgers later, and register the account_tx queries.
pub fn register_device(chain: &mut Chain, device: &Wallet, bundle: &DeviceBundle) -> Hash32 {
    let bytes = bundle.encode();
    let (bundle_id, total) = publish_bundle(chain, device, MemoKind::DeviceChunk, &bytes, DEVICE_CHUNKS);
    let manifest = DeviceManifest {
        network: NETWORK, bundle_id, bundle_len: bytes.len() as u32, chunk_total: total,
        first_chunk_ledger: DEVICE_CHUNKS, last_chunk_ledger: DEVICE_CHUNKS, kind: bundle.kind, env: bundle.env,
        key_id: bundle.key_id, account_id: device.account_id(), reg_ledger_index: bundle.reg_ledger_index,
        reg_ledger_hash: bundle.reg_ledger_hash,
    };
    let tx = chain.add(device, MemoKind::Device.as_str(), &manifest.encode(), DEVICE_MANIFEST);
    chain.queries.push((device.account_id(), DEVICE_CHUNKS, Some(DEVICE_CHUNKS)));
    chain.queries.push((device.account_id(), DEVICE_MANIFEST, None));
    tx
}

/// A lookup node answering from a fixed list.
pub struct StaticNode {
    pub url: String,
    pub answer: Result<LookupResult, String>,
}

impl StaticNode {
    pub fn new(candidates: Vec<Refs>, min: u32, max: u32) -> Self {
        Self {
            url: "https://node.test".into(),
            answer: Ok(LookupResult { candidates, ranges: vec![galaxy_verifier::LedgerRange { min, max }], incomplete: false }),
        }
    }
}

impl LookupSource for StaticNode {
    fn endpoint(&self) -> &str {
        &self.url
    }

    async fn lookup(&self, _photo_id: &Hash32, _network_id: u32) -> Result<LookupResult, LookupError> {
        self.answer.clone().map_err(LookupError)
    }
}
