use std::{env, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"));
    for (input, default, embedded) in [
        (
            "GALAXY_IOS_POLICY",
            "../../policies/galaxy-pc-v1m.json",
            "GALAXY_BUNDLED_POLICY",
        ),
        (
            "GALAXY_IOS_NETWORK",
            "../../deployments/networks/xrpl-testnet.toml",
            "GALAXY_BUNDLED_NETWORK",
        ),
    ] {
        println!("cargo:rerun-if-env-changed={input}");
        let path = env::var_os(input)
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join(default));
        let path = path
            .canonicalize()
            .expect("iOS configuration file must exist");
        let path = path.to_str().expect("iOS configuration path must be UTF-8");
        println!("cargo:rerun-if-changed={path}");
        println!("cargo:rustc-env={embedded}={path}");
    }
}
