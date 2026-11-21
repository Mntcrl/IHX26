# ADR 0120: Bounded decoder property tests

## Context

Security review finding S-08 called for repeatable testing at the parsers that consume untrusted bytes. Example vectors cover known cases but cannot cover the combinations of lengths, field values, container structure and data handled across these interfaces.

## Decision

Use the non-publishable `galaxy-security-tests` workspace package, with only a test target and dev dependencies. Run `cargo test -p galaxy-security-tests --release` through the workspace slot wrapper. Proptest uses a fixed ChaCha seed and explicit case counts and does not write corpus or regression files. The tests do not need network access, a device, private captures, or proving keys.

- Run size-bounded arbitrary input, including format-prefixed input, and explicit oversize cases through wire memo and bundle decoders, the XRPL codec, PNG and JPEG, XMP, App Attest CBOR and embedded DER, GXRW/GXEV, and node lookup JSON.
- Use checked-in public protocol and ledger vectors, public App Attest verification material, and generated synthetic images, evidence and lookup pages to reach code that random byte strings rarely reach.
- For byte-canonical encodings, require successful single-byte mutations to re-encode to exactly the mutated bytes. For ordinary XML/JSON or receipt-stripping CBOR, check the parsed model under reserialization: their interchange encodings are not byte-canonical. JPEG is lossy and has no canonical encoding; test its metadata and raster separately and use lossless PNG for raster round-trips. Raw capture ingest quantizes and has no inverse; check validated evidence and image decode consistency instead. The App Attest counter hint is structural and does not authenticate anything.
- Count total Rust heap allocation requests (including reallocations) during each measured decoder call on the calling test thread. Use ceilings based on input limits, fixed decoded dimensions and overhead for parsed representation and library scratch space. Verify rejection at documented encoded-size boundaries separately; an encoded-size limit is not necessarily a heap-byte limit.

## Consequences

The suite catches panics, unexpected allocations and canonicality regressions for the cases it executes. It is not exhaustive or a substitute for coverage-guided fuzzing, sanitizer runs, external allocation tracking or dedicated worst-case resource testing. The allocation meter does not count stack use, other threads, allocator bookkeeping or allocations outside Rust's global allocator, and it detects budget overruns after allocation rather than enforcing a production limit. The suite uses finite fixture and random-input budgets to keep release-mode execution short.
