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
