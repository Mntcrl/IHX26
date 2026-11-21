# ADR 0108 — Request budgets

**Status:** Accepted

## Context

Public lookups and peer lists can be requested repeatedly. A peer exchange can advertise more hosts than a node can reasonably probe.

## Decision

Rate-limit the API by the TCP peer, or by a single address forwarded by an explicitly trusted proxy. Count IPv4 addresses and IPv6 /64s individually, with additional shared quotas at IPv4 /24 and IPv6 /56 and /48 sized as fixed shares of the global quota. Retain aggregate usage for the entire global window, regardless of client-table eviction. Bound concurrent cache reads and cap peer discovery at 64 origins and two peer-exchange hops per cycle. Backfill and live sync share a configured XRPL call budget; proof verification has a separate configured budget.

## Consequences

An overloaded node can decline requests without asserting anything about the photo. A single tracked prefix cannot exhaust a node-wide quota greater than one request per window. When the bounded aggregate table fills, previously unseen prefixes must wait for the next window. Trusted proxies must replace any client-supplied forwarding header. An upstream RPC provider shared by the two budgets must be provisioned for their combined traffic.
