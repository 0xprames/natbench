# Transport delivery queue

Continue shipping independently useful development slices and previews while
outside evaluation for stable v0.2.0 is pending. Current public binaries are
v0.2.0-alpha.2; they do not include the new network-control inputs below.

## 1. Network conditions

- [x] First development slice: matched directional delay/random loss in comparison
  input 2; reusable link controls in application input 4; preserved kernel
  configuration/counters, partial setup and cancellation evidence.
- [ ] Follow-up slices: bounded jitter, bandwidth and MTU controls, composed with
  NAT profiles; declare scope and supported capabilities without silently dropping
  an unsupported request.
- [ ] Publish a preview after native distribution/install validation.

First-slice acceptance: real iroh/Quinn workloads prove upload and download delay,
verified delivery with loss, and explicit failures under total loss. Old inputs
remain supported; invalid controls fail early and fixture errors stay distinct
from transport outcomes. Kernel evidence survives failure/interruption.

## 2. TCP baseline

Add a third separately built adapter using the same fresh-payload verification
and reliable-stream workload. Record kernel/socket settings and authentication
semantics. Keep plain TCP's security differences visible; comparable workload
semantics do not imply equal security or pure implementation overhead. Demonstrate
successful TCP delivery when UDP is blocked, and actual failure under applicable
network loss. Include the executable in native archives.

## 3. Regression gates and saved comparisons

Add explicit requirements for delivery rate, p95 application RTT and minimum
verified goodput with sample-count requirements. Failed/unsupported/incomplete
attempts stay visible and cannot become passing results through filtering. Saved
baseline comparisons require compatible workloads, topology, settings and network
conditions; distinguish relative change from an absolute requirement. Leave noise
budgets explicit instead of promising identical timings across hosts.

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
