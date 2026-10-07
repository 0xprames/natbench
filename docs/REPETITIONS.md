# Repeated scenarios and timing summaries

The v0.2.0-alpha.1 preview accepts `natbench repeat SCENARIO --runs N --artifacts DIRECTORY`.
The default is five runs; 1–100 are allowed. Built-in input version 1 and application
versions 2 and 3 work with the same command. Validate prerequisites with `doctor`
before using the privileged Linux fixture.

```sh
rustc --edition 2021 examples/udp-recovery/main.rs -o examples/udp-recovery/udp-recovery
cargo build --locked
sudo ./target/debug/natbench repeat examples/udp-recovery/scenario.json --runs 5 --artifacts ./repetitions-001
```

The runner validates once and freezes the input JSON. It retains the original
application working directories and launches new programs in fresh network fixtures
for every case of every run. Application binaries, filesystem state and inherited
environment are the caller's execution context; files and dependencies may change
between attempts. Reset application state in your scenario when it matters.

All planned runs continue after completed assertion, fixture or inconclusive
verdicts. Every attempt contributes to the recorded counts. Cancellation or an
unexpected runner/artifact error stops repetition and preserves available evidence.
Existing artifact directories are refused. The parent directory must exist.

## Artifacts and completion

The top-level directory contains the captured `scenario.json`, `summary.json`,
and aggregate `junit.xml`. Each attempted run uses `run-001`, `run-002`, etc. with
its own scenario snapshot, suite report 2, JUnit, and available application logs
and timelines. Raw observations remain in these per-run reports; the summary keeps
verdicts, messages, durations and relative artifact directory names.

The [version 1 repetition schema](../schemas/repetition-report-v1.schema.json)
uses `kind: "repetition_report"`. `requested_runs` is the budget; `completed_runs`
counts runs whose entire case plan and suite evidence completed. `complete` says
all requested runs did that, including completed runs with failed cases. Success
requires exit 0, `complete: true` and `interrupted: false`.

The summary checkpoints before and after each run. `active_run` identifies the
attempt in progress; its per-run suite report gives current case-level progress.
An interrupted attempt enters `runs` with its completed case prefix and active
case. Available checkpoints are also retained after an unexpected runner error.
The aggregate JUnit keeps planned case slots for every requested run, including
skipped future slots, and adds runner-error/interruption testcases as needed.

Each case summary has these disjoint counts:

- `completed_runs`: the sum of its four completed verdict counts under `verdicts`.
- `interrupted_runs`: it was the active case when that run reported interruption.
- `unfinished_runs`: a checkpoint recorded it active without a completed verdict
  when a runner error ended the attempt.
- `unreported_runs`: remaining planned slots, including future/current attempts
  awaiting aggregation and slots skipped after cancellation or a runner error.

These counts sum to the case's `requested_runs`. Interrupted and unfinished slots
have no completed case-time sample. Run indices and records form a one-based prefix;
each run's completed cases form the original case-plan prefix. `active_run`, when
present, is the next index after the recorded runs. The schemas describe shape;
these ordering and count relationships are additional semantic requirements.

JSON is authoritative if a crash leaves JUnit at an older checkpoint. Files are
replaced atomically one at a time. SIGKILL or power loss can leave an active,
incomplete checkpoint. A signal after all cases finish can produce both
`complete: true` and `interrupted: true`; cancellation still exits 130.

| Exit | Meaning |
| --- | --- |
| 0 | All requested runs completed and every case passed |
| 1 | At least one completed assertion failed |
| 2 | Invalid input, artifact/runner error, or a completed fixture failure |
| 3 | Incomplete repetition or an inconclusive case |
| 130 | Cancellation |

Precedence is cancellation, fixture/runner failure, incomplete/inconclusive,
assertion failure, then success. A failed attempt continues to affect the exit
status after later attempts pass.

## Timing and environment

`timing_seconds` groups case samples by verdict. Every group reports sample count,
minimum, maximum, mean, median and p95. An empty group is omitted. For even sample
counts the median averages the two middle values. P95 uses nearest rank:
the sorted sample at one-based index `ceil(0.95 * count)`; with fewer than 20 samples
it is the maximum. These are observed summaries, with no confidence interval or
guarantee about future success rates.

Case time includes fixture setup, execution/assertions and fixture teardown.
Suite time includes input artifact writes and suite checkpoints, while excluding
aggregate summary writes. `completed_run_seconds` includes all complete suite
attempts with saved evidence, including failed verdicts. An unfinished run retains
its elapsed attempt time in its run record. Application-level connection latency
requires application measurements; these times describe the end-to-end test cost.

`environment` records natbench version, OS, architecture, kernel release when
available, effective UID, and an observation timestamp in Unix seconds. Application
versions and host load need their own recorded context for comparisons. Programs
inherit the normal scenario execution context described in [application scenarios](APPLICATIONS.md).
The same captured scenario can produce different timings or allocation observations
on different kernels and machines.

Application repeats accept [`--capture`](PACKETS.md) or `--capture=N` for bounded
packet evidence in every attempted case. `environment.packet_capture` records the
options. Capture startup, shutdown and validation contribute to case times.
