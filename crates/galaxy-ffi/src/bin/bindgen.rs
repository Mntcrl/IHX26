fn main() {
    if std::env::args().nth(1).as_deref() == Some("check-circuit") {
        let mut args = std::env::args_os().skip(2);
        let Some(descriptor) = args.next().filter(|_| args.next().is_none()) else {
            eprintln!("usage: galaxy-ffi-bindgen check-circuit DESCRIPTOR");
            std::process::exit(2);
        };
        let configured = std::fs::read(descriptor)
            .map_err(|_| "descriptor could not be read")
            .and_then(|bytes| {
                galaxy_ffi::CaptureCircuit::configure(bytes)
                    .map_err(|_| "descriptor, policy, and network allowlist do not agree")
            });
        match configured {
            Ok(circuit) => println!(
                "{}: {}",
                circuit.network().circuit_registration_tx,
                circuit.ceremony()
            ),
            Err(err) => {
                eprintln!("iOS circuit check: {err}");
                std::process::exit(1);
            }
        }
        return;
    }
    uniffi::uniffi_bindgen_main();
}
