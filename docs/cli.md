# `galaxy` command-line tool

Binary of `crates/galaxy-cli`; all verification logic is the shared
`galaxy-verifier` library (see [protocol/verification.md](protocol/verification.md)).

```
cargo build -p galaxy-cli --release            # target/release/galaxy
cargo build -p galaxy-cli --release --features node   # adds `galaxy node …` (galaxy-node library)
```

## Global options

| option | meaning |
|---|---|
| `--network-config PATH` | network configuration TOML (spec §12: trusted circuits, app IDs, attestation environments, inception ledger). Default: the built-in copy of `deployments/networks/xrpl-testnet.toml`. |
| `--rpc URL` (repeatable) | XRPL JSON-RPC endpoints tried in order. Default: the config's `trusted_local_rippled` (if set), then `json_rpc`. HTTPS required except loopback HTTP. Reports `rpc_trust: local` when every endpoint is loopback or the configured local rippled. |
| `--recording FILE` | replay recorded XRPL answers (offline, deterministic; unanswered requests → UNAVAILABLE). |
| `--record FILE` | save every XRPL answer of this run to FILE (replay later with `--recording`). |
| `--json` | print the `galaxy-report/v1` JSON (or the command's JSON document) instead of text. |
| `--timeout SECS` | overall time limit for network work (default 300); exceeding it yields UNAVAILABLE. |

## Commands

| command | network | output |
|---|---|---|
| `verify <image> [--manifest-tx H [--proof-tx H]] [--lookup] [--node URL…] [--no-discovery]` | XRPL (+ lookup nodes) | report |
| `lookup <photo_id> --node URL…` | lookup nodes only | untrusted candidate hints; nothing verified |
| `inspect <manifest_tx>` | XRPL | proof-only report (image binding not checked) |
| `proof verify <proof_tx>` | XRPL | proof-only report for that exact PROOF |
| `circuit inspect <circuit_id>` | XRPL (circuit registration) | descriptor, policy, digests, ceremony — only for circuits accepted by the config |
| `policy inspect <policy_id\|file>` | file: none; id: XRPL via its accepted circuit | policy id, name, windows, policy JSON |
| `photo id <image>` | none | photo_id, rgb_root, container profile |
| `node run\|sync\|status\|peers` | (feature `node`) | delegates to galaxy-node: `run --config P`, `sync --config P`, `status --config P \| --url U`, `peers --config P \| --url U` |

### `verify` modes

- **direct** (default): references from the image's Galaxy XMP; they are hints,
  every binding is checked against XRPL.
- **manual**: `--manifest-tx H` (and optionally `--proof-tx H`, which is evaluated
  and listed but can never select itself).
- **lookup**: `--lookup` asks the nodes given with `--node` (or `GALAXY_NODES`,
  comma-separated) for the photo ID, then verifies every hint from XRPL. Without
  `--lookup`, an image without references falls back to lookup when nodes are
  configured.
- `--no-discovery`: never contact a lookup node (the photo ID stays local); an
  image without references then reports NOT_FOUND with "no capture was
  consulted". There is no built-in node list.

Lookup endpoint (node REST API): `GET {node}/v1/photos/{photo_id}?network_id=N`
→ `{"candidates": [{"manifest_tx", "proof_tx"|null}], "ranges": [{"min", "max"}],
"incomplete": bool}` (≤ 1 MiB, no redirects, 15 s timeout).

## Exit codes

| code | meaning |
|---|---|
| 0 | VERIFIED (or a non-report command succeeded) |
| 10 | PENDING |
| 20 | INVALID |
| 30 | INCONCLUSIVE |
| 40 | NOT_FOUND |
| 50 | UNAVAILABLE (also: `lookup` with no node answering) |
| 60 | UNSUPPORTED (also: `circuit`/`policy inspect` of a non-accepted circuit or policy) |
| 2 | usage error (clap errors, invalid hashes or URLs, `--lookup` without nodes) |
| 1 | internal error (unreadable files, invalid network config, runtime failure) |

## Examples

```
# Testnet, public RPC, no lookup nodes contacted
galaxy verify photo.png --no-discovery
galaxy --json inspect 3F0C…E1
# Own rippled
galaxy --rpc http://127.0.0.1:5005/ verify photo.jpg --manifest-tx 3F0C…E1
# Record a session, replay it offline
galaxy --record session.json verify photo.png
galaxy --recording session.json verify photo.png
# Offline vectors shipped with the repository (development credential → exit 60)
galaxy --recording test-vectors/verifier/dev-unattested/recording.json \
       --network-config test-vectors/verifier/dev-unattested/network.toml \
       inspect "$(jq -r .manifest_tx test-vectors/verifier/dev-unattested/refs.json)"
```
