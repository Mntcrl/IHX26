# Browser verifier

The Galaxy photo verifier is a static site. It runs the same Rust verification pipeline as the `galaxy` CLI, compiled to WebAssembly and executed in a Web Worker. It does not require an account, wallet, backend, or upload.

## Build, run and test

Use the repository's pinned Rust toolchain with `wasm32-unknown-unknown`, Node.js 22.12+ (22, 24, or 26+) and npm. The Rust `wasm-bindgen-cli` must be exactly **0.2.130**; its install includes the WASM test runner. The script prefers `.tools/bin` and falls back to `PATH`.

```sh
rustup target add wasm32-unknown-unknown
scripts/with-slot.sh cargo install --locked wasm-bindgen-cli --version 0.2.130 --root .tools
make verifier-build  # generates bindings/wasm/pkg/ and apps/verifier-web/dist/
make verifier-run    # serves the built site on http://127.0.0.1:4173/ (Python 3)
make verifier-test
```

Set `WEB_PORT` for another local port. `make verifier-run` uses the existing build if present; run `make verifier-build` to update it. `GALAXY_WEB_NETWORK_CONFIG` overrides the built-in `deployments/networks/xrpl-testnet.toml` at build time. The browser build requires XRPL Testnet (network ID 1); it embeds the config's accepted circuits, app IDs and attestation environments. Settings can change network **endpoints**, not the acceptance policy. After registering a new accepted circuit, update the operator's network config and rebuild. An empty circuit allowlist cannot produce `VERIFIED` for any circuit.

Copy the contents of `apps/verifier-web/dist/` to any HTTPS static host; a subdirectory with a trailing-slash URL works. Serve `.wasm` as `application/wasm`, JavaScript files with a JavaScript MIME type, and `index.html` as HTML. Do not open `index.html` with `file://`. Use a current browser with WebAssembly, module and Blob workers, and `Promise.withResolvers`. No rewrite rules or server-side verifier processes are required. Asset sizes (including gzip) are printed by `make verifier-build`.

### Run the verifier on a laptop

Unzip the presenter package (or put the built `dist/` and `serve-local.sh`
together in a folder). On the laptop, open Terminal in that folder and run
`./serve-local.sh` with Python 3 installed. Open **http://127.0.0.1:8080/** in a
current browser. The server accepts only local connections; leave Terminal
open while verifying, then press Ctrl+C to stop it.

On the iPhone, open **Gallery**, select the certified capture, and choose
**Certified JPEG · original → Share JPEG → AirDrop**. Don't resave the file
in Photos or a photo editor. On the laptop, choose the JPEG, keep **Embedded
Galaxy references** selected, and click **Verify locally**. The photo stays
on the laptop; an internet connection is needed to read XRPL Testnet.

If the package includes `galaxy`, run `./galaxy verify "/path/to/photo.jpg"
--no-discovery` on an Apple Silicon Mac to verify the same file from Terminal.

## Use and interpret the result

1. Choose or drop one JPG or PNG (maximum 16 MiB). The verifier accepts the versioned Galaxy PNG/JPEG profiles, not every image that a browser can preview.
2. Keep **Embedded Galaxy references** selected if the image carries Galaxy XMP. This preferred mode works with discovery disabled.
3. If the references are absent, enter the **Manual manifest transaction** hash or enable **Photo ID via Galaxy nodes**. Manual mode searches bounded XRPL account history for the associated proof. For photo-ID lookup, add node base URLs in settings, enable the option, and acknowledge the privacy notice for each verification. Nodes return untrusted hints; the browser verifies the candidate transactions and their image bindings independently.
4. Select **Verify locally**. The structured report shows the exact verification state, individual checks, XRPL references and ledger coverage, attestation environment, claimed versus ledger-close UTC times, and limitations. Circuit-proven properties and committed-only Bluetooth and noise statistics are marked separately. A missing or unsupported proof does not establish that an image was manipulated, and `VERIFIED` establishes only physical consistency under the named policy.

The WebAssembly package can also be used outside the app: `inspect_image(bytes)` returns the local container, profile, photo ID, RGB root, untrusted XMP and decoding errors; `verify(bytes, options, ledgerSource, nodeClient?)` returns `galaxy-report/v1` JSON. The JS ledger methods (`tx`, `ledger_header`, `account_tx`, `server_info`) receive XRPL parameters as JSON text and return the bare XRPL result as JSON text. The optional JS node callback supplies capped raw `/v1/photos/:photo_id` pages; Rust validates and paginates them using the shared protocol parser. Discovery requires an explicit consent option and is not used in direct or manual mode.

## Network and CSP

- No image bytes, filename, witness material or sensor data are included in network requests. There are no analytics, external fonts, remotely loaded scripts or third-party assets. The only localStorage data is settings: XRPL URLs, node URLs, the discovery switch and explorer templates. Consent and image selection are not saved.
- For XRPL the defaults are Testnet WebSocket and HTTPS JSON-RPC fallback. An RPC provider sees your IP/browser connection details and may see the transaction hashes, account lookups and ledger indices you query. WebSocket is preferred; an HTTPS provider must permit the browser origin via CORS. Remote RPC endpoints are trusted for validated ledger history, ordering and completeness. Use your own synchronized, CORS-enabled rippled if you want to control that source. Plain HTTP/WS is allowed only on `localhost` or `127.0.0.1`; use a hostname rather than an IPv6 literal for compatibility with CSP.
- A node sees your IP and photo identifier **only after** you choose the photo-ID mode and consent to the listed nodes for that specific verification. Nodes can correlate repeated lookups, omit candidates, or provide incomplete coverage. Discovery is off by default; while off, no discovery fetch is made. Opening an explorer link is a separate, explicit navigation and discloses the referenced public transaction or ledger to that provider.
- The HTML installs a restrictive static CSP for code and assets (`default-src 'none'`, `script-src 'self' 'wasm-unsafe-eval'`, `style-src 'self'`, `worker-src blob:`). The app then installs a second policy **before any file is read or worker is created**. Its `connect-src` lists the exact same-origin WASM asset, configured XRPL URLs and, only if enabled, the configured nodes' `/v1/photos/` paths. The policies intersect. The Blob worker inherits the page's policies; saving settings reloads the document with the new allowlist. HTTPS and WSS are accepted; a host-level header may further restrict allowed endpoints but cannot widen the page's policy. Do not use a header that disallows Blob workers or WASM compilation if you intend to run this build.
- CSP is defense in depth, not proof against a compromised host or browser/extension. Serve the generated files unmodified from a trusted HTTPS origin. The JS transport never passes image bytes to native network APIs; fetch rejects redirects, omits credentials, and streams capped bodies. A WebSocket message is size-checked before JSON parsing; the browser may buffer the whole frame before exposing it to the worker.

## Offline and browser verification

`make verifier-test` covers the capped transports and URL checks (Vitest), golden JPEG/PNG identities on native and on WASM, and genuine Groth16 proof verification and changed-RGB rejection on WASM. It recreates the synthetic dev-unattested photo, then compares the whole CLI and WASM reports for direct and manual lookup against the same recorded ledger; no timing fields currently need normalization. This fixture is deliberately **UNSUPPORTED**, not a certified photograph.

To repeat the Chromium smoke test against the same recording (requires matching Chromium and ChromeDriver, and does not use the public Testnet):

```sh
mkdir -p target/VerifierWeb
CARGO_TARGET_DIR=target/VerifierWeb scripts/with-slot.sh cargo run --locked -p galaxy-wasm \
  --example verifier_vector_image -- target/VerifierWeb/dev-photo.png
GALAXY_WEB_NETWORK_CONFIG=test-vectors/verifier/dev-unattested/network.toml make verifier-build
node apps/verifier-web/tests/browser-smoke.mjs target/VerifierWeb/dev-photo.png
make verifier-build  # restore the normal deployment config after testing
```

The smoke test serves the production bundle from a subdirectory, drags the image into Chromium, exercises manual and consented photo-ID lookup with an untagged copy, and checks the rendered report and network requests. The `CHROMIUM` and `CHROMEDRIVER` environment variables override the executable paths.
