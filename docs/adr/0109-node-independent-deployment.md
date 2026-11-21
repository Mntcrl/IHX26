# ADR 0109 — Independent deployment and test faults

**Status:** Accepted

## Context

A demonstration must show independent indexes and identities, as well as a verifier's behavior when a node returns wrong or missing references.

## Decision

Ship a multi-stage OCI build with digest-pinned base images, a non-root runtime user, and a local HTTP health check. The Compose example gives three nodes distinct state volumes, origins, and one bootstrap each. Build faulty local test nodes only with the opt-in `test-faults` feature, which production images omit.

## Consequences

The nodes can be removed or rebuilt independently. Fault injection changes only photo lookup output; it cannot alter the ledger cache or become a release-image setting.
