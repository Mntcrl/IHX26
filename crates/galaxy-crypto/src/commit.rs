//! Tree, scalar, padding and aggregate evidence commitments (§6).
use crate::{poseidon, tags::{self, tag}, Fr};
use ark_ff::AdditiveGroup;

pub fn tree_component(slot: u8, salt: Fr, root: Fr) -> Fr {
    assert!((1..=3).contains(&slot));
    poseidon::hash(&[tag(tags::COMPONENT, u64::from(slot)), salt, root])
}

pub fn scalar_component(slot: u8, salt: Fr, elements: &[Fr]) -> Fr {
    assert!((4..=8).contains(&slot));
    let mut h = poseidon::hash(&[tag(tags::SCALARS, u64::from(slot)), salt, Fr::from(elements.len() as u64)]);
    for group in elements.chunks(4) {
        let mut input = [Fr::ZERO; 5];
        input[0] = h;
        input[1..1 + group.len()].copy_from_slice(group);
        h = poseidon::hash(&input);
    }
    h
}

pub fn pad_component(slot: u8) -> Fr {
    assert!((9..=15).contains(&slot));
    poseidon::hash(&[tag(tags::PAD_EVIDENCE, u64::from(slot)), Fr::ZERO])
}

pub fn evidence_root(components: &[Fr; 16]) -> Fr {
    let mut inputs = [Fr::ZERO; 5];
    inputs[0] = tag(tags::EVIDENCE_NODE, 2 << 16);
    for (i, c) in components.chunks_exact(4).enumerate() {
        inputs[i + 1] = poseidon::hash(&[tag(tags::EVIDENCE_NODE, (1 << 16) | i as u64), c[0], c[1], c[2], c[3]]);
    }
    poseidon::hash(&inputs)
}
