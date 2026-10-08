# Changelog

## 0.3.0-alpha.1

Evaluation preview of the first network, TCP, regression-gate, recovery and
adapter-starter slices. Native x86-64/ARM64 bundles include all four example
executables. Outside evaluation and remaining M3 gates stay open.

- Application input 5 schedules declared link changes with fresh per-case context,
  atomic transition markers and monotonic/kernel evidence, including partial failure
  and cancellation. Separately built iroh/Quinn/TCP examples verify fresh data on
  the existing connection after outages and preserve persistent-outage failures.
  Native bundles and CI include the recovery binary, example and verifier.

- An independent Rust adapter generator copies source, locked dependencies and the
  small protocol into a separate project with a replaceable TCP transport module
  and CI workflow. Native bundles include its executable and generator.
- `conform` checks one adapter's lifecycle/contract and real workload bounds behind
  preserve/random NAT, plus explicit total-loss failures. Unsupported and incomplete
  checks remain nonpassing; cancellation/stale-output tests preserve diagnostics.

- Offline `assess` gates every comparison cohort against explicit delivery-rate,
  p95 application RTT, minimum verified-goodput and sample requirements. Saved
  baselines require matched experiments/machine/settings with explicit recorded
  build-version allowances; raw inputs, coverage and JSON/JUnit stay available.
- Comparison report 1 adds case workload/deadline and CPU/parallelism metadata.
  Native archives ship the example policy and regression verifier; CI assesses
  real comparison reports and separately labelled synthetic diagnostic cases.

- A separately built plain TCP baseline joins iroh/Quinn in direct and shaped
  comparisons. The same fresh message/bulk verification runs on bounded
  length-prefixed exchanges, with kernel/socket, stream and security metadata.
  TCP succeeds through blocked UDP; all three fail under directional total loss.
  Native archives include the updated executable and rebuildable source.

- Comparison input 2 declares matched client-to-server/server-to-client delay and
  random loss. Application input 4 exposes the same controls on named fixture links
  through `test`/`repeat`; older closed input versions retain their behavior.
- Per-case kernel qdisc settings/counters survive success, setup/process failure and
  cancellation. Changed final settings fail the fixture; comparison report 1 adds
  requested case conditions without mixing failed attempts into performance samples.
- Real adapter verification exercises directional delay, combined loss and total loss
  in both directions; native archive CI preserves packet and network evidence.

## 0.2.0-alpha.2

Transport comparison and real iroh connectivity evaluation preview. Stable v0.2.0
still requires an outside evaluation and resolution of blocking feedback.

- Native x86-64/ARM64 archives include standalone adapter/connectivity executables
  under `bin/` and their source. The shipped comparison and iroh verifier run without
  Rust installed; native archive CI checks this with Rust excluded from PATH.
- A copyable transport workflow downloads the pinned bundle and preserves comparison
  and connectivity evidence. Install/evaluation docs cover applications and transports.
- All three executables report the preview version; tested tag publication uses the
  repository's explicit release notes when present.

- Standalone real iroh relay/address-discovery and two-NATed-peer scenarios verify
  automatic/forced paths, blocked-UDP relay delivery, fresh data after relay shutdown
  on an established direct connection, and expected relay-dependent delivery failure.
  A verifier checks selected-path/application evidence and live-fixture cancellation;
  Linux/x86-64/ARM64 CI and extracted source archives exercise the cases.

- `compare` runs matched direct reliable-stream workloads through external executable
  adapters, rotating order and retaining per-attempt JSON/JUnit/logs/timelines/PCAPs.
- Independent pinned iroh/Quinn adapters verify fresh message and bulk payload bytes;
  reports keep conditional application timings/goodput, raw samples and build/settings
  metadata. Failures, unsupported coverage, invalid output and interruption stay distinct.
- Versioned request/event/bootstrap and comparison schemas support generic external
  implementations without linking their transport libraries into the core.

## 0.2.0-alpha.1

Application runner evaluation preview; M2 stable launch awaits planned transport
comparison adapters/metrics and outside evaluation.

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
