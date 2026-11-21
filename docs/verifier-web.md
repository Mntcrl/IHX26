# PHØTON browser verifier

The PHØTON photo verifier is a static site. It runs the same Rust verification pipeline as the `galaxy` CLI, compiled to WebAssembly and executed in a Web Worker. It does not require an account, wallet, or upload.

The file drop target is on the first screen, including on phones. **How it works** opens an optional explanation of the ledger challenge and 16-tile sample; the sample drawing is illustrative and never uses your photo. **Choose a photo** returns to the file input. Reduced motion shows the complete drawing without pinning the page.

## Build, run and test

Use the repository's pinned Rust toolchain with `wasm32-unknown-unknown`, Node.js 22.12+ (22, 24, or 26+) and npm. The Rust `wasm-bindgen-cli` must be exactly **0.2.130**; its install includes the WASM test runner. The script prefers `.tools/bin` and falls back to `PATH`.

```sh
rustup target add wasm32-unknown-unknown
scripts/with-slot.sh cargo install --locked wasm-bindgen-cli --version 0.2.130 --root .tools
make verifier-build  # generates bindings/wasm/pkg/ and apps/verifier-web/dist/
make verifier-run    # serves the built site on http://127.0.0.1:4173/ (Python 3)
make verifier-test
```

Set `WEB_PORT` for another local port. `make verifier-run` uses the existing build if present; run `make verifier-build` to update it. `GALAXY_WEB_NETWORK_CONFIG` overrides the built-in `deployments/networks/xrpl-testnet.toml` at build time. The browser build requires XRPL Testnet (network ID 1); it embeds RPC URLs, explorer templates, accepted circuits, app IDs and attestation environments. `PHOTON_WEB_LOOKUP_NODE` can replace the built-in demo node at build time. After registering a new accepted circuit, update the operator's network config and rebuild. An empty circuit allowlist cannot produce `VERIFIED` for any circuit.

Copy the contents of `apps/verifier-web/dist/` to any HTTPS static host; a subdirectory with a trailing-slash URL works. Serve `.wasm` as `application/wasm`, JavaScript files with a JavaScript MIME type, and `index.html` as HTML. Do not open `index.html` with `file://`. Use a current browser with WebAssembly, module and Blob workers, and `Promise.withResolvers`. No rewrite rules or server-side verifier processes are required. Asset sizes (including gzip) are printed by `make verifier-build`.

The scroll presentation bundles the MIT-licensed [ScrollCraft engine](https://github.com/nateherkai/scroll-craft/tree/75d81f74e83692add18cd7a8a8e078b8a887a579/plugins/nateherk-design/skills/scroll-craft/engine) locally. No design skill, browser harness, or art-generation tooling is required to build or run the verifier.

### Run the verifier on a laptop

Unzip the presenter package (or put the built `dist/` and `serve-local.sh`
together in a folder). On the laptop, open Terminal in that folder and run
`./serve-local.sh` with Python 3 installed. Open **http://127.0.0.1:8080/** in a
current browser. The server accepts only local connections; leave Terminal
open while verifying, then press Ctrl+C to stop it.

On the iPhone, open **Gallery**, select the certified capture, and choose
**Certified JPEG · original → Share JPEG → AirDrop**. Don't resave the file in
Photos or a photo editor. On the laptop, choose the JPEG, wait for the PHØTON
metadata badge and photo ID, then click **Verify photo**. The photo stays on
the laptop; an internet connection is needed to read XRPL Testnet.

If the package includes `galaxy`, run `./galaxy verify "/path/to/photo.jpg"
--no-discovery` on an Apple Silicon Mac to verify the same file from Terminal.

## GitHub Pages

Run `scripts/web.sh pages` to build `apps/verifier-web/dist/` for
[mntcrl.github.io/IHX26/](https://mntcrl.github.io/IHX26/). This sets the Vite
base to `/IHX26/` via `PHOTON_WEB_BASE`; the normal build uses relative asset
paths. Run `make verifier-build` again before serving at `/` with `make verifier-run`.

At the root of the `gh-pages` branch, copy the contents of `dist/`, add an empty
`.nojekyll`, and record the source `main` commit in a `README.md`. Never copy
photo fixtures, reports or screenshots. Commit the generated site and push it
with `git push origin gh-pages`. In the [repository settings](https://github.com/Mntcrl/IHX26/settings/pages),
go to **Pages → Deploy from a branch**, then choose **gh-pages** and **/ (root)**.

Certified photos with embedded references use live XRPL Testnet directly. The
optional photo-ID lookup uses the build's configured demo node; if its tunnel
URL changes, set `PHOTON_WEB_LOOKUP_NODE` to the new HTTPS origin and rebuild.

## Use and interpret the result

1. Choose or drop one JPG or PNG (maximum 16 MiB). Local inspection shows the copyable photo ID and whether Galaxy XMP references are present and consistent, absent, or unreadable. The badge is not a verification result. Only the versioned Galaxy PNG/JPEG profiles have a protocol photo ID; an image that a browser can preview may still be unsupported.
2. Click **Verify photo**. With embedded references, the browser reads XRPL directly; no lookup node is contacted. With no usable references, the default searches the disclosed PHØTON node by photo ID. Turn off **Look up by photo ID** to keep the ID local. With lookup off and no references, `NOT_FOUND` means nothing was consulted, not that the capture is absent. A parsed packet whose image binding is inconsistent is still verified directly so that the binding can fail explicitly.
3. **Advanced** lets you supply the CAPTURE transaction hash instead. This skips the node and searches bounded XRPL account history for the proof. When lookup is used, the node supplies untrusted hints; the browser verifies candidate transactions and image bindings independently.
4. Expand the grouped checks and details for XRPL transactions, ledger coverage, circuit/ceremony, attestation environment, claimed versus ledger-close UTC times, and limitations. Circuit-proven properties and committed-only Bluetooth and noise statistics are marked separately. `VERIFIED` establishes only physical consistency under the named policy, not scene truth. Missing, incomplete, or unsupported evidence does not establish manipulation.

The WebAssembly package can also be used outside the app: `inspect_image(bytes)` returns the local container, profile, photo ID, RGB root, untrusted XMP and decoding errors; `verify(bytes, options, ledgerSource, nodeClient?)` returns `galaxy-report/v1` JSON. The JS ledger methods (`tx`, `ledger_header`, `account_tx`, `server_info`) receive XRPL parameters as JSON text and return the bare XRPL result as JSON text. The optional JS node callback supplies capped raw `/v1/photos/:photo_id` pages; Rust validates and paginates them using the shared protocol parser. Discovery requires an explicit consent option and is not used in direct or manual mode.

## Network and CSP

- Local inspection requests only the app's own WASM asset; it never contacts XRPL or a lookup node. No image bytes, filename, witness material or sensor data are included in network requests. There are no analytics, external fonts, remotely loaded scripts or third-party assets. File selection, settings and lookup preferences are not persisted.
- The default XRPL sources are Testnet WebSocket and an HTTPS JSON-RPC fallback. An RPC provider sees your IP/browser connection details and may see transaction hashes, account lookups and ledger indices. An HTTPS provider must allow the browser origin via CORS. Remote RPC endpoints are trusted for validated history, order and completeness. To use your own synchronized, CORS-enabled rippled, set it in the build-time network config. Plain HTTP/WS endpoints are allowed only on `localhost` or `127.0.0.1`; IPv6 literals are not supported by the CSP allowlist.
- A lookup node sees your IP and photo ID only when you click **Verify photo** on a photo with no usable references while lookup is on. The exact destination host is disclosed next to the switch; the node can correlate queries or omit candidates. `?node=https://your-node.example` selects an alternative CORS-enabled HTTPS origin (without a path, credentials, query or fragment). Invalid or repeated overrides are ignored. Opening an explorer link is a separate explicit navigation and discloses the referenced public transaction or ledger to that provider.
- The static CSP restricts assets (`default-src 'none'`, `script-src 'self' 'wasm-unsafe-eval'`, `style-src 'self'`, `img-src 'self'`, `worker-src blob:`). Before reading files or creating a worker, the app installs a second `connect-src` policy listing only the app's WASM asset, configured XRPL URLs, and the validated node's `/v1/photos/` path. The policies intersect; the Blob worker inherits both. The switch gates creation of the node client separately from the CSP. Host-level headers may further restrict endpoints (including query-selected nodes) but cannot widen the policy. Do not serve this build with a header disallowing Blob workers or WASM compilation.
- CSP is defense in depth, not proof against a compromised host, browser, or extension. Serve the generated files unmodified from a trusted HTTPS origin. Browser transports reject redirects, omit credentials and cap response bodies. A WebSocket message is size-checked before JSON parsing; the browser may buffer the whole frame before exposing it to the worker.

## Offline and browser verification

`make verifier-test` covers the capped transports and URL checks (Vitest), golden JPEG/PNG identities on native and on WASM, and genuine Groth16 proof verification and changed-RGB rejection on WASM. It recreates the synthetic dev-unattested photo, then compares the whole CLI and WASM reports for direct and manual lookup against the same recorded ledger; no timing fields currently need normalization. This fixture is deliberately **UNSUPPORTED**, not a certified photograph.

To run the Chromium smoke test against the same recording (requires matching Chromium and ChromeDriver; no public Testnet calls):

```sh
mkdir -p target/VerifierWeb
CARGO_TARGET_DIR=target/VerifierWeb scripts/with-slot.sh cargo run --locked -p galaxy-wasm \
  --example verifier_vector_image -- target/VerifierWeb/dev-photo.png
node apps/verifier-web/tests/browser-smoke.mjs target/VerifierWeb/dev-photo.png
```

The smoke test builds into a temporary directory with the recording's configuration, serves the bundle from a subdirectory, tests drag/drop, local inspection and copying, then checks direct, manual, lookup opt-out and default lookup paths, grouped checks, and CSP. It also checks keyboard order, the first-screen drop zone on desktop and at 390 px, and the unpinned reduced-motion diagram. The `CHROMIUM` and `CHROMEDRIVER` environment variables override the executable paths.

For live acceptance after a production `make verifier-build`, pass `--live <certified.jpg> <stripped.jpg> <ordinary.jpg> <one-pixel.png> <screenshot-dir>` instead. Use an existing private directory for screenshots: they can show photo identifiers. Never add private fixtures, reports or screenshots to the presenter package.

### Scroll presentation checks

For a visual review, use the pinned skill's `shoot.mjs` from a separate build folder with `playwright-core` installed there. In a **test-only copy** of the harness, set `bypassCSP: true` in the new Playwright page options (the contrast pass masks text and samples a data URL) and include `[data-contrast]` in its contrast measurement and masking selectors. These markers let the harness measure static text without applying ScrollCraft's worldflight styling. Keep the verifier CSP unchanged and run `browser-smoke.mjs` separately with CSP enabled.

Run the visual harness against the production build on desktop, `--width 390 --height 844`, and `--reduced-motion`. Inspect the contact sheets as well as `report.json`: the optional diagram should lift the sample tiles and hold the completed contact sheet. The keyboard and motion checks in `browser-smoke.mjs` do not depend on ScrollCraft's harness.
