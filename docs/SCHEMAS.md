# Scenario and report compatibility

Scenario inputs, suite reports, raw experiment observations and doctor reports
have independent schema versions. The binary version is not a schema version.
Tools must inspect `schema_version` before interpreting JSON and reject versions
whose semantics they do not support. A failed parse is never a successful test.

## Scenario version 1

The runner accepts JSON inputs described by
[scenario-v1.schema.json](../schemas/scenario-v1.schema.json). All declared fields
are required and unknown fields are rejected at every input object level.
Cases must have nonempty, unique names; the runner enforces uniqueness across the
suite. A suite has 1–100 cases and each case has at least one expectation.
Timeouts are finite numbers greater than zero and no greater than 3600 seconds.
JSON Pointer escaping uses `~0` for `~` and `~1` for `/`; pointers start with `/`.
The empty pointer that addresses the entire document is not accepted.

Expectations compare complete JSON values by equality, including types. A missing
field is inconclusive; a present null is an observed value and can equal null.
Numeric integer and floating-point representations are not normalized: `1` and
`1.0` compare differently. The runner does not apply numeric tolerances.
Unknown fields, unsupported versions and invalid input fail before the artifact
directory is created or a network fixture is started.

Existing version 1 inputs retain their semantics. A new field or changed existing
behavior requires a new input version. Future runners adding a new input version
must retain the existing version 1 path or explicitly announce its retirement.
Validation errors are preferable to silently ignoring misspelled instructions.

## Application scenario version 2

[scenario-v2.schema.json](../schemas/scenario-v2.schema.json) adds external processes
and sequential lifecycle steps. Version 1 built-in scenarios remain accepted by
`natbench test`. Application inputs use the closed object policy above. The runner
also validates process identity/reference consistency, lifecycle ordering, working
directories, IPv4 readiness addresses, NUL-free argv/env and bounded byte patterns.
See [application scenarios](APPLICATIONS.md) for execution and readiness semantics.
Input version 2 and suite report version 2 are independent formats.

## Application scenario version 3

[scenario-v3.schema.json](../schemas/scenario-v3.schema.json) retains version 2's
process model and adds `wait_stdout`, `wait_exit` and `delay`. Declare version 3
to use these steps; version 2 rejects them. A command paired with `wait_exit`
may complete after readiness while the scenario continues, and its assertions
are evaluated when collected. Service supervision remains active during waits.
Existing version 1 and 2 paths remain supported. A version 2 application scenario
can opt into version 3 by changing only its version field. Suite reports remain
at version 2. See [application scenarios](APPLICATIONS.md) for ordering and deadlines.

## Suite report version 2

Current reports use [suite-report-v2.schema.json](../schemas/suite-report-v2.schema.json).
Version 2 introduces partially completed reports. Version 1 reports were written
only after execution; consumers must explicitly add version 2 support before
reading live checkpoints. The historical shape remains documented by
[suite-report-v1.schema.json](../schemas/suite-report-v1.schema.json).

| Field | Meaning |
| --- | --- |
| `schema_version` | 2 for suite reports with checkpoint state |
| `scenario` | Input file path recorded by the runner |
| `planned_cases` | Unique names in input order |
| `cases` | Completed case reports in execution order; may be an empty prefix |
| `active_case` | Name being attempted, or null; retained if that attempt was interrupted |
| `complete` | Every planned case has a completed verdict |
| `interrupted` | The runner observed cancellation; never interpret this as a passing suite |

The completed cases form a prefix of `planned_cases`. An active case is the next
planned case and is absent from `cases`. `complete` is true exactly when completed
and planned counts match. If a signal arrives after the final case completed,
both `complete` and `interrupted` can be true; exit status is still 130. A signal
delivered after the final checkpoint can also cause exit 130 while that checkpoint
still says uninterrupted. Require both a passing completed report and exit 0.

Completed case status values remain `passed`, `assertion_failed`, `inconclusive`
and `infrastructure_failed`. `observation` contains the raw experiment JSON when
available and is null after a fixture error. Cancellation is suite-level state;
the active case does not acquire a misleading infrastructure-failure verdict.

The v0.2.0-alpha.1 preview adds optional `elapsed_seconds` on completed cases and a JUnit
`time` attribute. It measures the case's monotonic wall time from fixture setup
through teardown, excluding suite checkpoint writes. Older version 2 reports may
omit it. Cases interrupted before a completed verdict do not provide this sample.

Output readers must accept additional fields within a supported version and must
not infer completion from the number of currently available cases. Changing field
types, meaning, required fields or the set of verdicts requires a new report
version. Input objects are closed; output objects permit additive metadata.
The schemas define structure; prefix/order/count invariants above are additional
semantic requirements enforced by the runner.

## Checkpoints and exit status

After validating input, the runner copies `scenario.json` and saves initial
`report.json` and `junit.xml`. It checkpoints before starting each case and after
completing it. Every file replacement writes and syncs a temporary file before
renaming it over that run's prior checkpoint. Existing run directories are refused.

SIGINT and SIGTERM stop the active fixture, keep completed observations, mark the
report interrupted, and exit 130. JUnit includes all planned cases, an error for
an interrupted active case, and skipped entries for cases that never completed.
These reports are evidence, not instructions to resume execution.

Files are atomically replaced individually, not as a multi-file transaction.
JSON is authoritative if a crash leaves JUnit at an older checkpoint. SIGKILL,
power loss or an artifact I/O error may leave only the last saved state and may
leave a temporary file; they cannot reliably produce an interruption marker.
A live or abandoned checkpoint with `complete: false` is not a passing result.

| Exit | Meaning |
| --- | --- |
| 0 | Complete suite, all expectations passed |
| 1 | Complete suite with assertion failure |
| 2 | Invalid input, artifact I/O error, or fixture failure |
| 3 | Missing observation; also the report API verdict for an incomplete checkpoint |
| 130 | Cancellation, even when previously completed cases passed |

For complete, uninterrupted suites, fixture errors take precedence over missing
observations, which take precedence over assertion failures. Cancellation takes
precedence over other verdicts. Existing standalone measurement commands retain
their prior exit semantics.

## Other JSON and release migration

Application packet manifests use [capture schema 1](../schemas/packet-capture-v1.schema.json),
kind `packet_capture`. `capture-config.json` uses schema 1, kind
`packet_capture_options`, and the same nested options shape. Completed raw application
observations may embed a manifest under `packet_capture`; absent capture is null.
Repetition environment metadata may also include capture options. These additions
preserve input versions 1–3, suite report 2 and repetition report 1. See
[packet evidence](PACKETS.md) for scope, completion meaning and retained partial files.

`natbench repeat` writes [repetition report version 1](../schemas/repetition-report-v1.schema.json),
identified by `kind: "repetition_report"`. It is a separate document from the suite
reports retained under each `run-001`, `run-002`, etc. directory. Scenario inputs
remain at versions 1, 2 and 3; per-run suite reports remain at version 2. Aggregate
readers must dispatch by command/type and check `complete`, `interrupted` and the
process exit status. See [repetition semantics](REPETITIONS.md) for counters and timing.

Raw experiments embedded under `observation` keep their own `schema_version: 1`.
Doctor reports also remain at version 1. Report consumers must dispatch by both
command/document type and version; the same number does not identify a shared
format. Raw measurements describe this topology and time, not guaranteed future
reachability on another network.

The v0.1.1 release introduced suite report version 2 while continuing to accept
scenario version 1. Upgrade wrappers to check `complete` and `interrupted`, retain
case status handling, and continue treating a nonzero process exit as failure.
Consumers supporting older archives may keep their explicit version 1 reader.

Machine-readable schemas use the
[JSON Schema 2020-12 dialect](https://json-schema.org/draft/2020-12/json-schema-core).
They are shipped alongside the example scenario in release archives.

## Transport adapter and comparison formats

The v0.2.0-alpha.2 preview adds independent version 1 formats distinguished by `kind`:
`transport_comparison` input, `transport_request`, `transport_peer`,
`transport_event` and `transport_comparison_report`. Definitions are in `schemas/`.
Inputs are closed; output objects allow additive metadata. An echoed workload in
a measurement accepts additive output metadata without relaxing request validation.
Bounds, identity/order/count invariants, exit/event agreement and metric semantics
are documented in [the adapter guide](ADAPTERS.md). Comparison report completion
is independent of success: require exit 0, complete true and interrupted false.
Application inputs 1–3, suite report 2, repetition report 1 and capture manifest 1
retain their versions. Application exit timeline events gain an additive exit_code.

## Standalone iroh connectivity example

[Iroh connectivity requests](../schemas/iroh-connectivity-request-v1.schema.json)
and [JSONL events](../schemas/iroh-connectivity-event-v1.schema.json) use independent
schema 1 kinds `iroh_connectivity_request` and `iroh_connectivity_event`. Inputs
are closed; events accept additive metadata. The relay has null run_id/role/policy;
peer events identify the attempt and role. Path snapshots and verified byte/sequence
assertions are described in [the example guide](IROH_CONNECTIVITY.md). This is an
example-specific format, not an extension of the direct comparison contract.
[Peer bootstrap](../schemas/iroh-connectivity-peer-v1.schema.json) uses kind
`iroh_connectivity_peer`, schema 1. The verifier writes
[the suite summary](../schemas/iroh-connectivity-suite-v1.schema.json), kind
`iroh_connectivity_suite`, schema 1, with separate expected outages and a cancellation
result; require its exit 0 and complete true.
Core scenario/report versions retain their existing semantics.

## Development network-control inputs

[Comparison input 2](../schemas/transport-comparison-v2.schema.json) requires
explicit directional conditions on every case. [Application input 4](../schemas/scenario-v4.schema.json)
requires a per-case set of named egress links and retains input 3 lifecycle steps.
Existing comparison input 1 and application inputs 2/3 reject the new `network`
field, including null. The closed input policy and bound/uniqueness checks remain
in effect. Core suite report 2, repetition report 1 and comparison report 1 retain
their versions; comparison reports add optional `case_conditions`, and application
observations add optional `network_conditions` metadata.

[Network evidence 1](../schemas/network-conditions-v1.schema.json) is distinguished
by kind `network_conditions`; completion concerns collection/configuration integrity,
independently from the test outcome. See [network semantics and limits](NETWORK_CONDITIONS.md).
The v0.2.0-alpha.2 archive predates these formats.

## Development regression assessment

[Regression policy 1](../schemas/transport-regression-policy-v1.schema.json) has
kind `transport_regression_policy` and closed, bounded inputs. [Assessment report
1](../schemas/transport-regression-report-v1.schema.json) has kind
`transport_regression_report`, permits additive output metadata, and retains source
coverage, recomputed measurements, verdicts and compatibility changes. Comparison
report 1 adds optional per-case workload/deadline descriptors; environment metadata
adds CPU identity and available parallelism. Older output readers can ignore these
additions. Assessment needs the descriptors, and baseline comparison needs machine
metadata; rerun older reports with current source. No existing input version changes.
See [requirements, baseline compatibility and exits](REGRESSION_GATES.md).

## Development adapter conformance

[Adapter config 1](../schemas/transport-adapter-config-v1.schema.json) has kind
`transport_adapter_config` and one closed executable declaration. The generated
[plan 1](../schemas/transport-adapter-conformance-plan-v1.schema.json), kind
`transport_adapter_conformance_plan`, is execution evidence. [Raw runs 1](../schemas/transport-adapter-conformance-runs-v1.schema.json)
uses kind `transport_adapter_conformance_runs`; [primary verdicts 1](../schemas/transport-adapter-conformance-report-v1.schema.json)
uses kind `transport_adapter_conformance_report`. Expected total-loss transport
failures remain raw failures while their conformance verdict passes. Unsupported
coverage and interruption never pass. Require the primary report/exit, and publish
its JUnit rather than the diagnostic nested outcome JUnit. Existing comparison
inputs still require 2–8 adapters; the generated conformance plan is a separate kind
and cannot be rerun as a comparison. See [the starter guide](ADAPTER_STARTER.md).

## Scheduled recovery inputs and evidence

[Application input 5](../schemas/scenario-v5.schema.json) adds `network_set` on
initially declared links and reserved per-case artifact/run-ID context. Inputs 2–4
reject this action. [Network evidence 2](../schemas/network-conditions-v2.schema.json)
retains initial/current state and partial or complete transitions; successful atomic
markers follow [transition 1](../schemas/network-transition-v1.schema.json).
[Reference recovery event 1](../schemas/transport-recovery-event-v1.schema.json)
records fresh post-restoration verification and same-connection evidence, or a
failed attempt without a timing. Suite report 2 remains authoritative.
See [recovery semantics](TRANSPORT_RECOVERY.md).
