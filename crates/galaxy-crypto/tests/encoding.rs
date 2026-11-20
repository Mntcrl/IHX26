use galaxy_crypto::{field::*, layout::*, poseidon, tags};
use serde_json::Value;

fn cases(text: &str) -> Vec<Value> {
    serde_json::from_str::<Value>(text).unwrap()["cases"].as_array().unwrap().clone()
}

#[test]
fn poseidon_matches_circomlib() {
    for case in cases(include_str!("../../../test-vectors/protocol/poseidon.json")) {
        let inputs: Vec<_> = case["inputs"].as_array().unwrap().iter().map(|s| fr_from_dec(s.as_str().unwrap()).unwrap()).collect();
        assert_eq!(fr_to_dec(&poseidon::hash(&inputs)), case["output"].as_str().unwrap());
    }
}

#[test]
fn field_limbs_tags_match_vectors() {
    let all = cases(include_str!("../../../test-vectors/protocol/field.json"));
    let case = &all[0];
    for v in case["scalars"].as_array().unwrap() {
        let bytes = hex::decode(v["bytes"].as_str().unwrap()).unwrap();
        let dec = v["decimal"].as_str().unwrap();
        let f = fr_from_be_canonical(&bytes);
        let g = fr_from_dec(dec);
        assert_eq!(f, g);
        assert_eq!(f.is_ok(), v["canonical"].as_bool().unwrap());
        if let Ok(f) = f {
            assert_eq!(fr_to_be(&f).as_slice(), bytes);
            assert_eq!(fr_to_dec(&f), dec);
        }
    }
    for v in case["limbs"].as_array().unwrap() {
        let bytes = hex::decode(v["bytes"].as_str().unwrap()).unwrap();
        let (hi, lo) = hash_limbs(bytes.as_slice().try_into().unwrap());
        assert_eq!(fr_to_dec(&hi), v["hi"].as_str().unwrap());
        assert_eq!(fr_to_dec(&lo), v["lo"].as_str().unwrap());
    }
    for v in case["tags"].as_array().unwrap() {
        let code = v["code"].as_u64().unwrap() as u16;
        let aux = v["aux"].as_str().unwrap().parse().unwrap();
        assert_eq!(fr_to_dec(&tags::tag(code, aux)), v["value"].as_str().unwrap());
    }
    assert_eq!(fr_from_i128(i128::MIN), -galaxy_crypto::Fr::from(1u128 << 127));
}

#[test]
fn field_decoders_reject_malformed_encodings() {
    for len in 0..128 {
        let bytes = vec![0xff; len];
        assert!(fr_from_be_canonical(&bytes).is_err());
        assert!(fr_from_dec(&"1".repeat(len)).is_ok() == (1..=77).contains(&len));
    }
    for bad in ["-0", "-1", "00", "01", " 1", "1 ", "+1", "1e1", "1.0", "𝟙"] {
        assert!(fr_from_dec(bad).is_err());
    }
}

#[test]
fn morton_slots_and_strata() {
    let mut slots = [false; 4096];
    for tile in 0..64 {
        let (tx, ty) = tile_xy(tile);
        assert_eq!(tile_label(tx, ty), tile);
        assert_eq!(CENTRAL_TILES.contains(&tile), (2..=5).contains(&tx) && (1..=4).contains(&ty));
        assert_eq!(PERIPHERAL_TILES.contains(&tile), tile < 48 && !CENTRAL_TILES.contains(&tile));
        for by in 0..8 {
            for bx in 0..8 {
                let slot = usize::from(leaf_slot(tile, bx, by));
                assert!(!std::mem::replace(&mut slots[slot], true));
            }
        }
    }
    assert!(slots.iter().all(|&u| u));
    assert_eq!(leaf_slot(47, 7, 7), 3583);
}
