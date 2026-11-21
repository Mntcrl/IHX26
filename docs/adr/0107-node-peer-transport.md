# ADR 0107 — Guarded outbound peer transport

**Status:** Accepted

## Context

Peer origins come from untrusted network data. An origin can resolve to a local address or attempt to redirect requests elsewhere.

## Decision

Allow HTTPS origins only by default. Block internal and special-use IPs both for literal hosts and for DNS answers passed to the HTTP connector. Do not follow redirects or proxies. Bound connection time and response bytes. Permit private destinations and HTTP only via an explicit local-test switch.

## Consequences

Public nodes cannot use this discovery path for local services. A private Compose network must opt in; it is not a production deployment configuration.
