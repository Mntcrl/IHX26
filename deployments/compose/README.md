# Testnet three-node deployment

From the repository root:

```sh
docker compose -f deployments/compose/compose.yaml up --build -d
docker compose -f deployments/compose/compose.yaml ps
for port in 8081 8082 8083; do
  curl -fsS "http://127.0.0.1:$port/v1/status"
  curl -fsS "http://127.0.0.1:$port/v1/peers"
done
```

All three nodes index XRPL independently, with separate volumes, SQLite caches, and Ed25519 keys. Each has one configured bootstrap on the private bridge. It learns the other via signed peer exchange, if that node is available. The advertised `node-a`, `node-b`, and `node-c` origins are only usable on the Compose network; the published loopback ports are for local clients. `/health` checks availability, not completeness. Inspect `indexed_ranges` and `gaps` in status to compare coverage.

In a bounded Testnet run over ledgers 22,483,819–22,483,831, every node independently returned the same public development CAPTURE and PROOF references, cached a passing Groth16 check without image binding, and discovered both other peers. The credential remained `UNSUPPORTED`, as required for development-unattested captures; the bounded index correctly reported incomplete global history.

The example is not a public topology: HTTP and private peer destinations are explicitly enabled. In a public deployment use HTTPS origins behind a reverse proxy; do not allow private peers. See [node operator docs](../../docs/node.md).

Use `GALAXY_NETWORK_CONFIG=/absolute/path/network.toml` to provide an alternative network/inception. For a bounded demonstration window, copy `deployments/node/node-{a,b,c}.toml` into a temporary directory, add the same `end_ledger` before `[limits]`, and set `GALAXY_NODE_CONFIG_DIR` to that absolute directory. Use a separate Compose project (`-p <name>`) for additional volumes. A capped range remains globally incomplete once Testnet advances beyond it.

To build or reuse a tag explicitly:

```sh
docker buildx build --platform linux/amd64 --load -f deployments/docker/Dockerfile -t galaxy-node:local .
# The build stage cross-compiles for arm64 when run on an amd64 builder.
docker buildx build --platform linux/arm64 --load -f deployments/docker/Dockerfile -t galaxy-node:arm64 .
docker compose -f deployments/compose/compose.yaml up --no-build -d
```

`docker compose ... down` stops the demo without removing volumes. Use `-v` only to discard all three node identities and caches. To reset only a cache, use `galaxy-node rebuild` with the daemon stopped.
