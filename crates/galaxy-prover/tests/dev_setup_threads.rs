//! Thread bound of `dev_setup`, in its own test binary so no other test's threads
//! share the process. ark-ec 0.6's parallel MSM builds a new rayon pool per call;
//! calling it per wire reached 621 live threads on this fixture (8 cores) and 16,458
//! on a 500k-constraint circuit. The setup must stay within the current rayon pool.
#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use galaxy_prover::dev_setup::dev_setup;

const DEV_PTAU_SHA256: [u8; 32] = [
    0x92, 0x18, 0xac, 0x8e, 0x64, 0x9a, 0x82, 0xb9, 0xef, 0x27, 0x23, 0xc8, 0x84, 0x23, 0x3e, 0xd9,
    0x08, 0x3b, 0xa2, 0x7f, 0x4e, 0xbe, 0xe1, 0x70, 0x9d, 0xd6, 0xac, 0xb5, 0x5b, 0x9d, 0xfd, 0x08,
];

#[test]
fn thread_count_stays_bounded() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-vectors/prover");
    let zkey = std::env::temp_dir().join(format!(
        "galaxy-prover-dev-setup-threads-{}.zkey",
        std::process::id()
    ));
    let pool = rayon::current_num_threads();
    let done = Arc::new(AtomicBool::new(false));
    let peak = Arc::new(AtomicUsize::new(0));
    let sampler = {
        let (done, peak) = (done.clone(), peak.clone());
        std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                let n = std::fs::read_dir("/proc/self/task")
                    .map(|d| d.count())
                    .unwrap_or(0);
                peak.fetch_max(n, Ordering::Relaxed);
                std::thread::sleep(Duration::from_micros(200));
            }
        })
    };
    let result = dev_setup(
        &dir.join("fixture.r1cs"),
        &dir.join("dev_9.ptau"),
        &DEV_PTAU_SHA256,
        &zkey,
    );
    done.store(true, Ordering::Relaxed);
    sampler.join().unwrap();
    let _ = std::fs::remove_file(&zkey);
    result.unwrap();
    // rayon pool + libtest main and test threads + sampler.
    let peak = peak.load(Ordering::Relaxed);
    let limit = pool + 4;
    assert!(
        peak <= limit,
        "peak {peak} live threads > {limit} (rayon pool {pool})"
    );
}
