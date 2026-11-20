# ADR 0049 — Compare the policy R1CS with reference GeometryViews

Status: accepted (CircuitGeo)

## Context
A PASS-only assertion can hide differences in intermediate decisions and counts. A witness calculator that refuses an input is not evidence of constraint-level rejection.

## Decision
Use a standalone `GalaxyPolicyV1m` harness and the Rust engine's expected report, checking all 19 statistics, all five predicates and their PASS conjunction. Exercise the coefficient-bound branch using a separately staged test-only policy. Check the R1CS on each honest witness and on targeted modifications covering boolean output, inlier, block coordinates, first maximum, IMU integration and variable floor division. Optionally build private views from real calibration captures using synthetic challenges and the same reference/harness comparison.

## Consequences
Equivalence failures identify the statistic or predicate rather than only the final verdict. The mutation tests establish rejection of the tested modified assignments; the arithmetic bounds explain why changing auxiliary wires cannot make a different quotient or comparison valid. Private views remain only under `fixtures/real/`, with failures reported without paths or values. Regenerate the public fixtures and rerun the comparisons when the policy changes.
