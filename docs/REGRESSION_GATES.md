# Gate transport delivery and performance

Current development source adds `natbench assess`. It reads saved comparison
reports without starting a fixture or requiring root, network tools, or adapters.
The published v0.2.0-alpha.2 predates this command and its report metadata.

```sh
cargo build --locked
cargo build --release --locked --manifest-path examples/transports/Cargo.toml
sudo ./target/debug/natbench compare examples/transports/direct.json --runs 3 --capture --artifacts ./comparison-001
./target/debug/natbench assess comparison-001/report.json --policy examples/transports/regression-policy.json --artifacts ./assessment-001
```

Use a new artifact directory with an existing parent for every assessment.
The [example policy](../examples/transports/regression-policy.json) covers all six
case/adapter cohorts. It requires three attempted and successful runs, at least
20 raw RTT samples, all deliveries, p95 message RTT at most 100 ms, and minimum
successful-attempt verified bulk goodput of 1 MiB/s. These are example requirements
for the declared direct fixture; choose requirements appropriate to your workload
and host. They are not published transport performance guarantees.

## Requirements and coverage

[Policy schema 1](../schemas/transport-regression-policy-v1.schema.json), kind
`transport_regression_policy`, is a closed input. Every case/adapter cohort in the
report needs exactly one rule. Filtering out a failed or unsupported cohort is
rejected. Each rule requires:

| Field | Meaning |
| --- | --- |
| `case`, `adapter` | Exact labels from the comparison |
| `min_attempts` | Minimum planned attempts, 1–100 |
| `min_successful_attempts` | Minimum verified successful attempts, 1–100 |
| `min_message_rtt_samples` | Minimum raw successful RTT samples, 1–100000 |
| `min_delivery_rate` | Successful deliveries divided by **every planned attempt**, 0–1 |
| `max_message_rtt_p95_seconds` | Optional absolute RTT maximum, positive and at most 60 seconds |
| `min_bulk_goodput_bytes_per_second` | Optional positive floor for the **minimum** goodput across successful attempts |

RTT pools successful message samples and uses nearest-rank p95. Samples within an
attempt are correlated; the counts are not a confidence interval. With fewer
than 20 samples, p95 becomes the maximum. Goodput divides receiver-verified payload
bytes by the bulk send/verify/ack interval. Message and goodput metrics remain
conditional on delivery; failed attempts have no completed metrics.

The assessor validates plan/attempt indices, complete/interrupted state, workload
identity, sample cardinality, finite positive timings, deadlines and bulk byte
counts. It recomputes metrics from raw attempts and ignores cached summaries.
Older reports lacking case workload/deadline metadata need a new run with current
source. The reader bounds each report to 128 MiB and the policy to 1 MiB; split
larger experiments. Input files are evidence claims, not signed attestations of
transport behavior; the real adapters and fixture verifiers establish delivery.

An explicit delivery-rate tolerance can pass a cohort with transport failures.
The failures retain their original outcomes and counts; the assessment also saves
the verdict implied by the original comparison report. It cannot recover an
external process exit or attest that every artifact write succeeded. `compare` still exits 1 for any transport
failure. In CI, preserve that report and assess it explicitly if your policy allows
loss; fixture/input errors, unsupported coverage and interruption must remain
nonpassing. A permissive delivery-rate threshold cannot hide insufficient samples,
unsupported attempts, fixture errors, missing attempts or cancellation.

## Compare a saved baseline

Add a `baseline` object to the applicable rules and supply `--baseline`:

```json
"baseline": {
  "max_delivery_rate_drop": 0.02,
  "max_message_rtt_p95_increase_fraction": 0.15,
  "max_bulk_goodput_drop_fraction": 0.15,
  "allowed_metadata_changes": []
}
```

```sh
./target/debug/natbench assess current/report.json --policy ./relative-policy.json --baseline baseline/report.json --artifacts ./regression-001
```

At least one relative requirement is necessary. Delivery-rate drop is an absolute
fraction: 0.02 permits a fall from 1.00 to 0.98. RTT increase is relative to baseline
p95; 0.15 permits at most 1.15 times baseline RTT. Goodput drop compares the minimum
successful-attempt goodput: 0.15 requires at least 0.85 times baseline goodput.
Absolute requirements still apply to current results. Sample-count requirements
apply to both reports for each relative rule; run counts may differ.

Baseline and current must contain exactly the same case/adapter cohorts. Recorded
workload, deadline, NAT profile, directional conditions, implementation name and
settings must match. Linux OS, architecture, kernel release, CPU model/identity,
available parallelism and capture options must also match. Missing machine
metadata blocks relative comparison. Explicit zero controls and undeclared controls
remain different recorded experiments. Timestamp, attempt IDs and ephemeral socket
addresses are not compatibility keys.

By default, implementation/build versions and fingerprints must match. When testing
a planned build or library upgrade, list the specific metadata changes you expect
under `allowed_metadata_changes`: `implementation_version`, `adapter_version`,
`adapter_source_sha256`, or `quic_engine_version`. Changes are printed and saved
with both values. This allowance does not relax workload, security, transport
settings, compiler, build profile or machine checks. Generic adapters can use these
build metadata names. Matching recorded settings cannot attest to unreported library
defaults or identical host load, CPU frequency, kernel scheduling or loss schedules.

Use repeated baselines on suitable hardware and set explicit allowances for expected
noise. These are deterministic gates over recorded samples, not statistical tests
or a causal attribution of performance changes. Keep the original reports and
their linked fixture evidence when investigating a failure.

## Assessment evidence and exits

[Assessment report 1](../schemas/transport-regression-report-v1.schema.json) has
kind `transport_regression_report`. The artifact directory contains exact frozen
`current.json`, `policy.json`, optional `baseline.json`, and `report.json`/`junit.xml`.
Frozen inputs use private file permissions. The report records the original
source-report locations; planned artifact paths resolve from their source report
directory, not from the assessment directory. Keep original evidence when moving
or sharing saved baselines. Each cohort keeps every planned attempt
index, all outcome/unreported counts, recomputed conditional distributions, checks,
and recorded compatibility differences. Source reports and their per-attempt
artifacts remain the authority for raw transport measurements.

`complete` means every policy cohort was assessed; it does not claim complete
transport coverage. Require assessment exit 0, complete true and interrupted false.
Interrupted/incomplete sources and unassessed cohorts cannot pass.

| Exit | Meaning |
| --- | --- |
| 0 | Every requirement passes with supported, complete coverage |
| 1 | Absolute/relative requirement or sample-count failure |
| 2 | Invalid input, artifact error, fixture error, changing identity/settings or incompatible baseline |
| 3 | Unsupported or incomplete coverage |
| 130 | Interrupted source or assessment |

JUnit includes all policy cohorts, including skipped unsupported/unassessed entries.
Skipped coverage keeps the CLI nonzero. Cancellation takes precedence, then errors,
unsupported/incomplete coverage and requirement failures. Invalid input fails before
creating artifacts; recorded-setting compatibility failures preserve assessment diagnostics. A missing
cohort or malformed matrix is a preflight error.

The [regression verifier](../scripts/check-regression-gates.sh) assesses a real
completed comparison with absolute requirements and a self-baseline. Labelled
synthetic report mutations exercise regression and incompatibility diagnostics;
they are not new transport runs or performance results. Linux and native
x86-64/ARM64 CI run this verifier, including packaged execution without Rust.
