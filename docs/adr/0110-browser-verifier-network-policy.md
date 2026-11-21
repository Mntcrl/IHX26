# ADR 0110: Browser verifier network policy is inherited by a Blob worker

## Context

The verifier must run image decoding and verification off the UI thread, serve without a backend, and support XRPL and optional Galaxy node endpoints selected in settings. A normal worker loaded from its own URL does not inherit the page's runtime CSP, and a static `connect-src` cannot enumerate per-user nodes. Discovery off and per-run consent must remain independent of networking availability.

## Decision

Vite embeds the module worker in the app bundle. The document installs both a static restrictive asset CSP and a runtime `connect-src` before spawning the worker. The Blob worker inherits both. Endpoint settings are validated before allowlisting; the allowlist contains only the WASM asset, configured XRPL URLs and, if discovery is on, configured node lookup paths. Saving settings reloads the document, since CSP only tightens for a loaded document. A fresh worker receives image bytes by transfer and is terminated after a report, cancellation or timeout. Only the per-run consented lookup constructs the node client; that client fetches raw, capped pages and shared Rust code validates every page and candidate.

## Consequences

The bundle works from static HTTPS hosting and a subdirectory; it requires a Blob-worker capable browser with CSP WebAssembly support. Operators can enforce a stricter host-level CSP at the cost of user-configurable endpoints. Routes, response caps and consent checks remain necessary independent defenses. The host and browser are trusted; CSP does not protect from a compromised served application or extension.
