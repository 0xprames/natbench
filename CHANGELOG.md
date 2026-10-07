# Changelog

## 0.2.0-alpha.1

Application runner evaluation preview; M2 stable launch awaits outside evaluation.

- Binary/source walkthrough reproduces a failed delivery requirement, diagnoses it
  from reports/logs/PCAPs, and verifies the corrected network scenario repeatedly.
- Copyable GitHub Actions workflow pins the preview, verifies download checksums,
  and preserves evidence on failure; contributor and outside evaluation guides.
- Release archives include the demo scenarios, verifier and Actions example; both
  native architectures exercise the extracted diagnostic demo before publication.

- Optional bounded `--capture` for application tests/repetitions, with private per-role
  PCAPs, manifest/logs, declared readiness and graceful flush before namespace cleanup.
- `repeat` runs a frozen scenario with fresh fixtures, retains every attempt, and
  checkpoints aggregate JSON/JUnit with verdict counts and timing distributions.
- Repetition reports record natbench/kernel versions, OS, architecture and caller UID;
  suite report 2 adds optional case wall times and JUnit time attributes.
- Application input schema 2 with ordinary executable argv, cwd/env, declared
  readiness, deadlines and sequential lifecycle steps.
- Per-launch stdout/stderr and JSONL lifecycle timelines, preserved on failure.
- Dedicated process groups terminate service children during stop/restart.
- Independent Go UDP scenario for NAT, restart and an expected blocked-UDP result.
- Application input schema 3 adds bounded stdout/exit waits, asynchronous command
  completion and supervised delay steps, preserving version 1 and 2 inputs.
- Independent Rust UDP client demonstrates fresh-data recovery on the same socket
  across a WAN service outage behind preserve and random NATs.

## 0.1.1

Published M1 release with tested x86-64 and ARM64 GNU/Linux archives and checksums.

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
