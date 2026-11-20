//! Circom-compatible Poseidon (§6). The backend retains its round parameters and scratch state.
use crate::Fr;
use light_poseidon::{Poseidon, PoseidonHasher};
use std::cell::RefCell;

thread_local! {
    static HASHERS: RefCell<[Option<Poseidon<Fr>>; 5]> = const { RefCell::new([None, None, None, None, None]) };
}

/// Hash one to five inputs. Panics on a programmer error (unsupported arity).
pub fn hash(inputs: &[Fr]) -> Fr {
    let arity = inputs.len();
    assert!((1..=5).contains(&arity), "Galaxy Poseidon requires 1..=5 inputs");
    HASHERS.with(|hashers| {
        hashers.borrow_mut()[arity - 1]
            .get_or_insert_with(|| Poseidon::<Fr>::new_circom(arity).expect("supported arity"))
            .hash(inputs)
            .expect("inputs match arity")
    })
}
