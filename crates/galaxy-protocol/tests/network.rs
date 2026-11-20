use galaxy_protocol::network::{NetworkConfig, NetworkError, NetworkId};
use galaxy_protocol::wire::AttestationEnv;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../deployments/networks/");

fn testnet_text() -> String {
    std::fs::read_to_string(format!("{ROOT}xrpl-testnet.toml")).unwrap()
}

#[test]
fn shipped_configs_load() {
    let testnet = NetworkConfig::load(format!("{ROOT}xrpl-testnet.toml")).unwrap();
    assert_eq!(testnet.network_id, NetworkId::Testnet);
    assert_eq!(testnet.network_id.as_u32(), 1);
    assert_eq!(testnet.name, "xrpl-testnet");
    assert!(!testnet.allow_mainnet);
    assert!(testnet.inception_ledger > 0);
    assert_eq!(testnet.max_fee_drops, 1000);
    assert_eq!(testnet.trusted_local_rippled, None);
    assert_eq!(testnet.accepted_app_ids, ["PPS57BF3X3.utcq.XRPL-Galaxy"]);
    assert_eq!(testnet.accepted_attestation_environments, [AttestationEnv::Development, AttestationEnv::Production]);
    assert!(testnet.accepted_circuits.is_empty());
    assert_eq!(
        testnet.explorer_tx_url(&[0xab; 32]),
        format!("https://testnet.xrpl.org/transactions/{}", "AB".repeat(32))
    );
    assert_eq!(testnet.explorer_ledger_url(7), "https://testnet.xrpl.org/ledgers/7");

    let devnet = NetworkConfig::load(format!("{ROOT}xrpl-devnet.example.toml")).unwrap();
    assert_eq!(devnet.network_id, NetworkId::Devnet);
}

#[test]
fn accepted_circuits_parse() {
    let hex = |b: &str| b.repeat(32);
    let text = testnet_text().replace(
        "accepted_circuits = []",
        &format!(
            "[[accepted_circuits]]\ncircuit_id = \"{}\"\nvk_digest = \"{}\"\npolicy_id = \"{}\"\nregistration_tx = \"{}\"\n",
            hex("01"),
            hex("aB"),
            hex("03"),
            hex("04")
        ),
    );
    let config = NetworkConfig::from_toml(&text).unwrap();
    let circuit = config.accepted_circuit(&[1; 32]).unwrap();
    assert_eq!(circuit.vk_digest, [0xab; 32]);
    assert_eq!(circuit.registration_tx, [4; 32]);
    assert!(config.accepted_circuit(&[2; 32]).is_none());

    let duplicated = text.clone() + &text[text.find("[[accepted_circuits]]").unwrap()..];
    assert!(NetworkConfig::from_toml(&duplicated).is_err());
    let short = text.replace(&hex("03"), &hex("03")[..62]);
    assert!(NetworkConfig::from_toml(&short).is_err());
}

#[test]
fn rejects_invalid_configs() {
    let base = testnet_text();
    let mainnet = base.replace("name = \"xrpl-testnet\"", "name = \"xrpl-mainnet\"").replace("network_id = 1", "network_id = 0");
    assert!(matches!(NetworkConfig::from_toml(&mainnet), Err(NetworkError::MainnetRefused)));
    let allowed = mainnet.replace("allow_mainnet = false", "allow_mainnet = true");
    assert_eq!(NetworkConfig::from_toml(&allowed).unwrap().network_id, NetworkId::Mainnet);

    for (from, to) in [
        ("network_id = 1", "network_id = 2"),
        ("network_id = 1", "network_id = 3"),
        ("https://s.altnet", "ftp://s.altnet"),
        ("wss://s.altnet", "https://s.altnet"),
        ("max_fee_drops = 1000", "max_fee_drops = 0"),
        ("max_fee_drops = 1000", "max_fee_drops = 1000001"),
        ("transactions/{hash}", "transactions/"),
        ("ledgers/{index}", "ledgers/{index}/{index}"),
        ("[\"development\", \"production\"]", "[\"not_applicable\"]"),
        ("[\"development\", \"production\"]", "[\"development\", \"development\"]"),
        ("[\"development\", \"production\"]", "[\"staging\"]"),
        ("[\"PPS57BF3X3.utcq.XRPL-Galaxy\"]", "[\"no-dot\"]"),
        ("allow_mainnet = false", "allow_mainnet = false\nunexpected = 1"),
        ("# trusted_local_rippled = \"http://127.0.0.1:5005/\"", "trusted_local_rippled = \"file:///etc\""),
    ] {
        assert!(base.contains(from), "{from}");
        assert!(NetworkConfig::from_toml(&base.replacen(from, to, 1)).is_err(), "{to}");
    }
    let inception = base.lines().filter(|l| !l.starts_with("inception_ledger")).collect::<Vec<_>>().join("\n");
    assert!(NetworkConfig::from_toml(&inception).is_err());
    let local = base.replace("# trusted_local_rippled", "trusted_local_rippled");
    assert_eq!(NetworkConfig::from_toml(&local).unwrap().trusted_local_rippled.as_deref(), Some("http://127.0.0.1:5005/"));
    assert!(matches!(NetworkConfig::from_toml(&"#".repeat(70_000)), Err(NetworkError::TooLarge)));
}

#[test]
fn truncated_configs_never_panic() {
    let text = testnet_text();
    for end in (0..text.len()).filter(|&i| text.is_char_boundary(i)) {
        let _ = NetworkConfig::from_toml(&text[..end]);
    }
}
