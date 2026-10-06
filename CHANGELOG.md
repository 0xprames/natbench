# Changelog

## 0.1.1

Prepared M1 release; publication follows merge, clean CI, and a version-matching
tag whose commit is reachable from `main`.

- Declarative built-in suites with explicit expectations, distinct CI verdicts,
  JSON/JUnit evidence, and artifact overwrite protection.
- `doctor` checks prerequisites and optionally probes a disposable kernel fixture.
- Native x86-64 and ARM64 GNU/Linux archives with checksums and extracted-binary tests.
- Atomic per-file checkpoints preserve completed results on SIGINT/SIGTERM, record
  interrupted active work, and mark unexecuted cases in JUnit.
- Published schema definitions and input/output compatibility rules.

Migration: `natbench test` now emits suite report schema version 2 because reports
can describe incomplete runs. Scenario inputs, raw experiments and doctor output
remain at schema version 1. Readers must check completion and interruption before
reporting a passing suite. See [schema compatibility](docs/SCHEMAS.md).

## 0.1.0

Experimental Linux namespace NAT observations, traversal and transport experiments.
