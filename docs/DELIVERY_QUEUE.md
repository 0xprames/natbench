# Transport delivery queue

Continue shipping independently useful development slices and previews while
outside evaluation for stable v0.2.0 is pending. Current public binaries are
v0.2.0-alpha.2; they do not include the development network controls or TCP baseline.

## 1. Network conditions

- [x] First development slice: matched directional delay/random loss in comparison
  input 2; reusable link controls in application input 4; preserved kernel
  configuration/counters, partial setup and cancellation evidence.
- [ ] Follow-up slices: bounded jitter, bandwidth and MTU controls, composed with
  NAT profiles; declare scope and supported capabilities without silently dropping
  an unsupported request.
- [ ] Publish a preview after native distribution/install validation.

First-slice acceptance: real iroh/Quinn/TCP workloads prove upload and download delay,
verified delivery with loss, and explicit failures under total loss. Old inputs
remain supported; invalid controls fail early and fixture errors stay distinct
from transport outcomes. Kernel evidence survives failure/interruption.

## 2. TCP baseline

- [x] Development baseline: separately built `--transport tcp` uses the same
  fresh-payload verification and reliable-stream workload as iroh/Quinn. Records
  kernel release, congestion control, socket snapshots, framing and no-TLS/
  no-authentication semantics; uses one fresh connection with `TCP_NODELAY`.
- [x] Verification: TCP delivers through blocked UDP while direct QUIC fails; all
  three deliver under directional delay/random loss and fail under total loss.
  Native archive CI tests the packaged executable and rebuildable source.
- [ ] Publish the TCP baseline and network controls in a validated native preview.

Comparable workload semantics do not imply equal security or pure implementation
overhead. Socket buffer values are snapshots because kernel autotuning can change
them; request/response framing differs between TCP and QUIC.

## 3. Regression gates and saved comparisons

- [x] Development gates: offline `natbench assess` checks delivery rate, p95
  message RTT, minimum verified goodput and explicit successful/sample counts.
  All cohorts and failed/unsupported/incomplete attempts stay visible.
- [x] Saved comparisons: explicit relative allowances, matched workload/topology/
  network/machine/settings metadata and audited build-version allowances.
  Raw reports/policy and JSON/JUnit are retained.
- [ ] Publish the command and example policy in a validated native preview.

See [gate semantics and saved-baseline guidance](REGRESSION_GATES.md). Relative
change and absolute requirements are separate; noise allowances are explicit.

## 4. Recovery experiments

Add scheduled network changes, outages, relay restart and NAT rebinding. Verify
fresh data after each event, measure the event-to-delivery interval and identify
whether the existing connection survived or a new connection was created. Preserve
failed recovery attempts, deadlines and actual path evidence. Reuse network-control
and application lifecycle foundations; include real transport scenarios.

## 5. External adapter starter and conformance checks

Provide a small independently built Rust adapter template and a checker for the
request/bootstrap/JSONL contract, lifecycle, failure/unsupported events and payload
verification. Run against a real fixture for delivery assertions; synthetic events
only validate parsing/orchestration. The implementation can live separately; no
private project source is required in this repository. Add a copyable CI example
and clear diagnostics for common integration mistakes.

Deliver each slice as a PR with scenario-level evidence. Merge after checks pass,
then alert the maintainer at a useful outcome. Outside feedback can change priority;
it does not require development to stop. Stable v0.2.0's evaluation gate remains
separate from these development tasks. See [the milestone plan](ROADMAP.md).
