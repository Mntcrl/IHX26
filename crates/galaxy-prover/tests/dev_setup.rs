//! `dev_setup` on the fixture R1CS with a small development ptau
//! (test-vectors/prover/dev_9.ptau, snarkjs `powersoftau new/contribute/prepare phase2`):
//! the delta-independent sections equal `snarkjs zkey new` byte for byte
//! (fixture_zkey_new.zkey), and the key proves and verifies.

use std::path::{Path, PathBuf};

use galaxy_prover::dev_setup::dev_setup;
use galaxy_prover::{verify, ProveControl, Prover, ProverError};

const DEV_PTAU_SHA256: &str = "9218ac8e649a82b9ef2723c884233ed9083ba27f4ebee1709dd6acb55b9dfd08";

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-vectors/prover")
}

fn digest(hex: &str) -> [u8; 32] {
    let mut d = [0u8; 32];
    for (i, b) in d.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap();
    }
    d
}

/// `(id, body)` of every zkey section.
fn sections(path: &Path) -> Vec<(u32, Vec<u8>)> {
    let b = std::fs::read(path).unwrap();
    let n = u32::from_le_bytes(b[8..12].try_into().unwrap());
    let mut pos = 12;
    (0..n)
        .map(|_| {
            let id = u32::from_le_bytes(b[pos..pos + 4].try_into().unwrap());
            let len = u64::from_le_bytes(b[pos + 4..pos + 12].try_into().unwrap()) as usize;
            pos += 12 + len;
            (id, b[pos - len..pos].to_vec())
        })
        .collect()
}

fn out_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "galaxy-prover-dev-setup-{tag}-{}.zkey",
        std::process::id()
    ))
}

#[test]
fn matches_snarkjs_zkey_new_and_proves() {
    let zkey = out_path("ok");
    let info = dev_setup(
        &dir().join("fixture.r1cs"),
        &dir().join("dev_9.ptau"),
        &digest(DEV_PTAU_SHA256),
        &zkey,
    )
    .unwrap();
    assert_eq!(
        (info.n_public, info.domain_size, info.ptau_power),
        (22, 512, 9)
    );

    let ours = sections(&zkey);
    let reference = sections(&dir().join("fixture_zkey_new.zkey"));
    let get = |s: &[(u32, Vec<u8>)], id: u32| s.iter().find(|(i, _)| *i == id).unwrap().1.clone();
    assert_eq!(
        ours.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
        (1..=9).collect::<Vec<_>>()
    );
    // Header up to gamma2 (n8q, q, n8r, r, nVars, nPublic, domain, alpha1, beta1, beta2, gamma2).
    let h = 4 + 32 + 4 + 32 + 12 + 64 * 2 + 128 * 2;
    assert_eq!(get(&ours, 2)[..h], get(&reference, 2)[..h]);
    assert_ne!(
        get(&ours, 2)[h..],
        get(&reference, 2)[h..],
        "delta is sampled, not the generator"
    );
    for id in [1, 3, 4, 5, 6, 7] {
        assert_eq!(get(&ours, id), get(&reference, id), "section {id}");
    }
    // C and H are divided by the sampled delta.
    for id in [8, 9] {
        assert_eq!(get(&ours, id).len(), get(&reference, id).len());
        assert_ne!(get(&ours, id), get(&reference, id), "section {id}");
    }

    let input = std::fs::read_to_string(dir().join("input.json")).unwrap();
    let prover = Prover::open(&dir().join("fixture.graph.bin"), &zkey).unwrap();
    let out = prover.prove(&input, &ProveControl::default()).unwrap();
    assert!(verify(&prover.vk_bytes().unwrap(), &out.public_inputs, &out.proof).unwrap());
    std::fs::remove_file(&zkey).unwrap();
}

#[test]
fn ptau_digest_mismatch_fails_closed() {
    let zkey = out_path("bad");
    let mut d = digest(DEV_PTAU_SHA256);
    d[31] ^= 1;
    let err = dev_setup(
        &dir().join("fixture.r1cs"),
        &dir().join("dev_9.ptau"),
        &d,
        &zkey,
    )
    .unwrap_err();
    assert!(
        matches!(&err, ProverError::InvalidArtifact(m) if m.contains("SHA-256")),
        "{err}"
    );
    assert!(!zkey.exists());
}

#[test]
fn circuit_larger_than_ptau_is_rejected() {
    // The 2^9 ptau cannot hold the fixture's domain when read as a 2^8 ceremony:
    // feed the r1cs to a ptau whose header power is lowered (digest pinned to the patched bytes).
    let mut ptau = std::fs::read(dir().join("dev_9.ptau")).unwrap();
    // Section 1 is first: magic(4) version(4) nSections(4) id(4) len(8) n8(4) q(32) power(4).
    let at = 12 + 12 + 4 + 32;
    assert_eq!(u32::from_le_bytes(ptau[at..at + 4].try_into().unwrap()), 9);
    ptau[at..at + 4].copy_from_slice(&8u32.to_le_bytes());
    let path = out_path("ptau8").with_extension("ptau");
    std::fs::write(&path, &ptau).unwrap();
    use sha2::{Digest, Sha256};
    let d: [u8; 32] = Sha256::digest(&ptau).into();
    let zkey = out_path("small");
    let err = dev_setup(&dir().join("fixture.r1cs"), &path, &d, &zkey).unwrap_err();
    std::fs::remove_file(&path).unwrap();
    assert!(
        matches!(&err, ProverError::InvalidArtifact(m) if m.contains("too big")),
        "{err}"
    );
    assert!(!zkey.exists());
}
