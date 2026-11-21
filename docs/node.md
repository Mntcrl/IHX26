# Galaxy verifier node

A node is a public, read-only XRPL index. It does not receive images, establish image binding, decide certification, or supply trust anchors. The CLI and browser verify lookup candidates using XRPL and their own configuration. A node's absence of results is bounded by its reported coverage.

## Run locally

```sh
make node-build
make node-run                       # 127.0.0.1:8080, Testnet, local cache under target/
make node-test
curl -fsS http://127.0.0.1:8080/v1/status
```

For another configuration set `GALAXY_NODE_CONFIG=/path/to/node.toml`. The executable also supports `galaxy-node run|sync|rebuild --config node.toml`, `galaxy-node status|peers --config node.toml`, `galaxy-node status|peers --url https://node.example`, and `galaxy-node health --url https://node.example`. The `galaxy` CLI exposes `galaxy node run|sync|status|peers` with `--features node`.

- Required: `network_config`, `data_dir`, `listen`, `public_url`. Paths are relative to the node TOML. The node URL is an origin (no credentials, query, fragment, or base path); use HTTPS and a TLS reverse proxy outside localhost. The service itself listens on plain HTTP.
- `sync_mode = "ledger"` (default) scans validated expanded ledgers from the network inception. `end_ledger` optionally stops indexing at that ledger; an older cap does not imply complete global history. `live = true` subscribes to ledger and transaction notices with reconnection; polling also runs when live subscriptions are disabled or unavailable. `rpc_endpoints` and `websocket_endpoints` are optional alternate configured sources. The configured trusted loopback rippled is tried first for RPC.
- `sync_mode = "account"` uses `accounts` (classic addresses) and/or `discover_accounts = true` (learn outbound publishers from confirmed live Galaxy notices). It never reports global coverage. `account_coverage` shows the indexed ledger ranges for individual scanned accounts; limit 64 accounts. Indexed references represent transactions published by that account, not all its incoming traffic. Keep account configuration and the corresponding cache together; rebuild when changing its intended scope.
- Peer discovery defaults on. `bootstrap_peers` starts discovery; `manual_peers` adds user-selected peers. Unexpired signed peers are cached locally. `xrpl_announcements = true` includes eligible `galaxy/v1/node` records the node indexed. Set `discovery = false` to disable outbound peer discovery; the local descriptor remains available. `allow_private_peers = true` allows private DNS/IP destinations and HTTP and is intended only for controlled tests (including Compose). Do not enable it on public deployments.
- `[limits]` HTTP defaults: `requests_per_minute = 180` per IPv4 address or IPv6 /64, and `global_requests_per_minute = 36000` across all clients. An IPv4 /24 or IPv6 /56 can spend at most 1/20 of the global quota (1,800 by default); an IPv6 /48 can spend at most 1/10 (3,600). Each applicable bucket must have capacity, and aggregate shares round down with a one-request minimum. The 4,096-entry client table evicts the least recently seen client when full. The separate 8,192-entry aggregate table retains admitted prefix counts until the node-wide 60-second window resets; if it fills first, requests needing a new aggregate bucket are refused until then.
- `[limits]` RPC defaults: `sync_requests_per_second = 4` shared between backfill and live sync, and `verify_requests_per_second = 4` reserved separately for proof verification. Both are configurable from 1 to 50 logical typed RPC calls per second; a call may also perform a NetworkID preflight. When both address the same endpoint, provision for their combined rate; separate node budgets do not reserve capacity at an upstream provider. Other defaults: `ledgers_per_pass = 128`, `poll_interval_secs = 5`, `discovery_interval_secs = 60`.
- Optional top-level `trusted_proxies = ["127.0.0.1", "::1"]` allows those exact TCP peer IPs to supply one bare client IP in `X-Forwarded-For`. The default is empty. Configure trusted proxies to **overwrite**, not append to, any client-supplied `X-Forwarded-For`. Missing, invalid, repeated, or comma-separated values use the TCP peer's bucket.
- `log_filter` defaults to `galaxy_node=info`; `RUST_LOG` overrides it for the standalone daemon. No request/photo IDs are logged by default.

## Demo node on this PC

For the PHØTON demo, build the release binary and start both Testnet profiles with the user systemd manager:

```sh
CARGO_TARGET_DIR=target/NodeOps1 scripts/with-slot.sh --heavy cargo build --locked --release -p galaxy-node
scripts/photon-node.sh start
scripts/photon-node.sh status
curl -fsS "$(scripts/photon-node.sh url)/v1/status"
```

The quick tunnel forwards HTTPS to a loopback ingress on port 8098; the ingress sends one validated `CF-Connecting-IP` as `X-Forwarded-For` and never passes a caller-supplied `X-Forwarded-For` to the node. The API is read-only, and only the tunnel is publicly reachable. This workstation uses `--protocol http2` because QUIC to Cloudflare timed out; HTTP/2 uses outbound TCP port 7844.

- The public route initially serves the account profile (port 8099). It scans the phone and desktop publishers and discovers additional accounts from live Galaxy notices. Its lookup can find a capture but does not establish global absence: an empty response has `incomplete: true`.
- The ledger profile (port 8100) scans from Testnet inception, ledger 22,480,444, with live updates. It fetches up to 16 historical ledgers concurrently under the shared sync budget and commits historical results in order. Run `scripts/photon-node.sh promote` once the ledger status reports `incomplete: false`. This switches the local route without changing the public origin or restarting `cloudflared`.
- State and runtime TOML files live under `target/photon-node/` (separate account and ledger databases). Do not publish the directory. The default rate is set in [`photon-demo.toml`](../deployments/node/photon-demo.toml). To change it, stop only `photon-node.service`, edit that rate, and run `scripts/photon-node.sh start ledger`; the database is reused.
- The units survive an operator shell or harness exit. Check `loginctl show-user "$USER" -p Linger` for logout behavior; transient units do not survive reboot. The tunnel does not automatically restart. `status` compares the running URL with the configured one and checks that the public endpoint responds. An explicit `scripts/photon-node.sh stop` also stops the tunnel; a subsequent `start` will get a new URL. Update the web verifier's default origin if this happens.

The first live backfill measured 9.34 contiguous historical ledgers/s over two minutes at the configured 20 logical RPC calls/s ceiling. Use the end of the range starting at 22,480,444 to measure historical progress; the highest indexed ledger can already be near the tip because of the live subscription.

## Cache and coverage

`index.sqlite3` and its SQLite sidecars are disposable. Stop the daemon before `galaxy-node rebuild --config node.toml`; it clears the cache and resynchronizes while preserving `node.ed25519`. The node identity is a private Ed25519 seed (owner-only on Unix), not a wallet key. Never publish the state directory.

Only validated `tesSUCCESS` transactions are considered. The XRPL client binds each record to its transaction hash and metadata position. A ledger's checkpoint and records commit together. Adjacent checkpoints must link by parent hash, including when live closes arrive before a historical gap is filled. History that a source does not advertise remains a gap. Account ranges are marked covered only after the terminal `account_tx` page for the requested interval; account markers are not passed between endpoints. A WebSocket notice does not prove index coverage: the node uses binary RPC reads and checks the noticed ledger hash before adding a live checkpoint. Status reports the latest validated tip, exact global ranges, gaps, and live connection/close counters separately. Conflicts are reported; the node does not rewrite checkpoints to fit a contradictory source.

Malformed Galaxy memos are retained locally as rejected, without serving them as candidates. Indexed references by themselves do not validate assertions, counters, credentials, or circuit bundles. Each PROOF gets an evaluation of that exact transaction via `galaxy-verifier`; cached `galaxy-report/v1` documents indicate `image_binding_checked: false`. Reports for a registration are invalidated and reevaluated when its revocation is observed, including when the node only discovers the affected CAPTURE during later backfill. A pending report is `null`, not a verifier verdict.

The worker tries due jobs in queue order, but after four consecutive jobs for one registration it selects a different due group if one exists. Groups are scoped by sending account and the device registration referenced by a matching same-account CAPTURE. Until that CAPTURE is indexed, the sending account alone acts as the group. A verification that times out (120 seconds) or returns a retryable state (`UNAVAILABLE`, `INCONCLUSIVE`, `NOT_FOUND`, `PENDING`) waits 60 seconds before its first retry; consecutive delays double up to six hours. Delays are stored per proof in SQLite and survive restarts. Revocations immediately invalidate cached reports but do not shorten existing retry delays. A change to the report build or trust configuration requeues all cached jobs with fresh delays.

## API and discovery

The [OpenAPI 3.1 document](../crates/galaxy-node/openapi.yaml) describes all endpoints and schemas. Typical lookup:

```sh
curl -fsS "http://127.0.0.1:8080/v1/photos/$PHOTO_ID?network_id=1&limit=50"
```

Results are ordered by ledger and transaction index. Follow a non-null `next_cursor` on the same endpoint and ID; coverage uncertainty (`incomplete`) is independent of pagination. Responses are bounded to 256 KiB, including application errors. Extremely fragmented coverage that cannot be returned within the bound fails closed; account and descriptor range summaries mark truncation explicitly. CORS permits cross-origin reads. `/health` checks local API/cache availability, not completeness or ledger trust.

`/v1/node` and `/.well-known/galaxy-node.json` return the same signed schema: `node_id` is the 32-byte Ed25519 public key (lowercase hex), and `signature` is the 64-byte signature (lowercase hex) over `GALAXY/NODE/v1` followed by a zero byte and the Galaxy canonical JSON of the descriptor without the signature. Its claims have no authority. The local descriptor expires in 15 minutes. Discovery validates compatible network, signature, expiry and URL, prohibits redirects and proxies, and checks destination addresses at DNS resolution (private, loopback, link-local and reserved addresses are prohibited by default). It probes at most 64 origins per cycle and follows up to two peer-exchange hops. Optional XRPL announcements also bind the fetched descriptor to the public key in the announcement. Discovery can be bypassed entirely by verifying explicit XRPL references.

## Test-only faults

`cargo build -p galaxy-node --features test-faults` allows a top-level `test_fault` in node TOML: `"wrong-references"`, `"omit-captures"`, or `"fail-lookup"` alter only photo lookup responses. Use separate local cache directories; the fault never modifies ledger or proof evaluation. A default/release image does not enable the feature and rejects the setting. A failed or incorrect node must not change what cryptographic verification establishes.

Run the reproducible two-minute public Testnet check with:

```sh
GALAXY_NODE_TESTNET_SMOKE=1 scripts/with-slot.sh cargo test -p galaxy-node live_testnet_smoke -- --ignored --nocapture
```

In one Testnet run, the initial scan indexed six ledgers (22,485,474–22,485,479). After 120 seconds live, the index had 44 contiguous checkpoints through ledger 22,485,517, with no gaps, 21 WebSocket ledger-close events, two subscription connections, and no Galaxy memos in that window. The recorded-fixture tests cover memo indexing and proof verification separately.

For containers, see [three-node Compose](../deployments/compose/README.md).
