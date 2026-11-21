# ADR 0105 — Bounded lookup with coverage

**Status:** Accepted

## Context

The CLI and browser need a shared contract for retrieving every known CAPTURE for a photo without interpreting an empty partial index as evidence of absence.

## Decision

Expose the protocol §14 JSON envelope and publish OpenAPI 3.1. Each lookup page carries global ranges and coverage status. Paginate references in ledger/transaction order, independently of whether history is complete, and cap a JSON response at 256 KiB. Return a structured error rather than advertise silently truncated global lookup coverage.

## Consequences

Clients follow `next_cursor` for additional candidates and treat `incomplete` separately. References and reported coverage are untrusted; the node need not receive the image.
