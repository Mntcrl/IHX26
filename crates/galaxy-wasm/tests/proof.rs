//! A real development main_v1m proof, including the public RGB commitment.
#![cfg(target_arch = "wasm32")]

use galaxy_crypto::{
    field::{fr_from_dec, fr_to_be},
    groth16::{verify, Proof, VerifyingKey},
};
use galaxy_protocol::wire::CircuitBundle;
use wasm_bindgen_test::wasm_bindgen_test;

const GXCB: &[u8] = include_bytes!("../../../test-vectors/verifier/main-v1m/circuit_bundle.bin");
const PROOF: &[u8] = include_bytes!("../../../test-vectors/verifier/main-v1m/proof.bin");
const INPUTS: &str = include_str!("../../../test-vectors/verifier/main-v1m/public.json");
const PHOTO: &[u8] = include_bytes!("../../../test-vectors/verifier/main-v1m/photo.png");

#[wasm_bindgen_test]
fn main_v1m_verifies_and_rejects_modified_rgb_commitment() {
    let bundle = CircuitBundle::decode(GXCB).unwrap();
    let key = VerifyingKey::from_bytes(&bundle.vk_bytes).unwrap();
    let proof = Proof::from_bytes(PROOF).unwrap();
    let mut inputs: Vec<_> = serde_json::from_str::<Vec<String>>(INPUTS).unwrap().iter()
        .map(|value| fr_from_dec(value).unwrap()).collect();
    assert_eq!(galaxy_verifier::photo_identity(PHOTO).unwrap().rgb_root, fr_to_be(&inputs[0]));
    assert!(verify(&key, &inputs, &proof).unwrap());
    inputs[0] += galaxy_crypto::Fr::from(1);
    assert!(!verify(&key, &inputs, &proof).unwrap());
}
