# ADR 0108 — Request budgets

**Status:** Accepted

## Context

Public lookups and peer lists can be requested repeatedly. A peer exchange can advertise more hosts than a node can reasonably probe.

## Decision

Rate-limit the API by the actual TCP source IP, bound concurrent cache reads, and cap peer discovery at 64 origins and two peer-exchange hops per cycle. The backfill, live index, and proof workers share a configured XRPL call budget.

## Consequences

An overloaded node can decline requests without asserting anything about the photo. Behind a reverse proxy, a shared source IP shares the same allowance; the node does not trust forwarded client-IP headers.
