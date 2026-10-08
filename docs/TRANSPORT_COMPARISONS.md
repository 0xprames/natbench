# Compare real transport implementations

Transport comparison is a primary milestone deliverable. A developer should be
able to ask how iroh, an independently built transport, and reference transports
deliver the same application workload under declared network conditions. CI runs
and retains these experiments; it is also useful to run them interactively.

The v0.3.0-alpha.1 preview implements the executable request/event contract,
standalone pinned iroh/Quinn adapters and a plain TCP baseline and `natbench compare` for matched direct
client-to-WAN workloads. See [usage, measurements and adapter integration](ADAPTERS.md).
[Local iroh relay/traversal tests](IROH_CONNECTIVITY.md) now run through application
scenarios with selected-path and verified-delivery evidence. [Scheduled link-outage recovery](TRANSPORT_RECOVERY.md) now verifies fresh data on
existing connections. Broader changing-network and generic cross-stack cohorts
below remain planned.
The older v0.2.0-alpha.1 predates these additions. Native bundles include the
adapter and connectivity executables alongside their source. Existing case wall times continue to
measure fixture execution, independently of adapter application timings.

## Two kinds of comparison

| Experiment | What stays common | What it answers |
| --- | --- | --- |
| Direct transport workload | Reachable endpoints, peer information, payloads, reliability requirements, network condition | Delivery, first-data latency, message RTT and verified goodput over a direct path |
| Complete connectivity stack | Peer placement, available signaling/relay services, workload and deadline | Discovery/traversal success, direct versus relayed delivery, fallback and recovery |

The preview includes iroh, Quinn and a plain TCP reliable-stream baseline
for direct workloads. TCP
records kernel/socket settings and its lack of encryption/authentication. Datagram
workloads form a separate cohort.
An external executable adapter lets a custom implementation enter the same
experiment without publishing its implementation or changing natbench's core.

For complete-stack tests, both peers sit behind declared routers and use their
normal discovery, traversal and fallback. Bare QUIC/TCP do not acquire a traversal
stack from the fixture. Declare unsupported capabilities explicitly, preserve
failed attempts, and keep capability coverage visible alongside conditional
performance samples. Compatible workload semantics do not imply wire-protocol
interoperability between these stacks.

## Adapter boundary

Keep adapters as ordinary executables, with Rust reference adapters in separately
built example crates. The core fixture should not need every transport library.
An adapter wraps the implementation's public API and exposes:

- A versioned workload/configuration request: endpoint role, peer/bootstrap
  information, payload sizes/counts, deadline, reliability mode and path policy.
- Explicit readiness and bounded, versioned events/results for connection
  attempts, verified messages, completion, path changes and errors. The first
  [file/JSONL contract](ADAPTERS.md) is implemented against working adapters.
- Implementation/build version, effective settings and supported capabilities.
  Missing metrics/capabilities remain explicit rather than becoming zero values.

The runner owns per-attempt state and artifact paths so identities, bootstrap
files and results cannot bleed between runs. Any out-of-band exchange of peer
information is declared and applied consistently. It never secretly punches
sockets or substitutes for the transport's own traversal in a complete-stack test.
Existing input versions remain supported when adding the comparison format.

## Workloads and results

Use identical workload definitions for each applicable adapter. Verify fresh
payload identifiers and bytes at the receiver, and acknowledge application
delivery where the workload requires it. Sent bytes and a successful handshake
alone do not establish delivery.

| Result | Measurement |
| --- | --- |
| Connectivity/delivery | Successful verified deliveries over all eligible attempts; preserve failure phase and deadline |
| First verified data | Adapter monotonic time from the declared dial/bootstrap start to an application acknowledgement |
| Message latency | Application echo/ack round-trip samples after a declared warm-up; report sample count and distributions |
| Goodput | Receiver-verified application bytes over the measured transfer interval, with integrity and completion status |
| Path selection | Available/selected direct or relay paths and changes, with implementation evidence and its limits |
| Recovery | Time from a declared outage/change to verified fresh delivery, identifying preserved versus recreated sessions |

Do not pool relayed and direct performance, reliable streams and unreliable
datagrams, cold and resumed connections, or successful and failed attempt times.
Failure counts remain part of the comparison; latency of successful attempts is
conditional on success. Missing one-way clock synchronization means reporting RTT,
not inventing one-way latency. Retain raw samples and per-attempt evidence.

Record implementation and compiler versions, workload parameters, transport
settings, topology, kernel, architecture, impairment configuration/seeds and path
policy. Use matched configurations and balanced/interleaved run order. Synthetic
single-host results describe this setup; public performance claims need repeated
runs on appropriate hardware. Resource metrics and overhead accounting can follow
once collection methods are defined consistently for endpoints and relays.

## Iroh integration

Pin an upstream release for the first adapter; [v1.3.0](https://github.com/n0-computer/iroh/releases/tag/v1.3.0)
is the initial research target. Iroh provides encrypted QUIC connectivity with
direct/relay behavior. Its endpoint builder supports explicit relay configuration
and an empty configuration without address lookup or relays; its connection API
exposes streams/datagrams and path snapshots/events.
[Endpoint configuration](https://github.com/n0-computer/iroh/blob/v1.3.0/iroh/src/endpoint.rs),
[connection and path APIs](https://github.com/n0-computer/iroh/blob/v1.3.0/iroh/src/endpoint/connection.rs).

First prove verified data in a labeled client-to-WAN direct-path experiment. Then
add two NATed peers using a real, locally configured iroh relay and the required
address-discovery configuration. Keep public relay/DNS services out of the isolated
fixture. Test automatic selection and forced relay policy independently. Record
actual path events; stop the relay and verify fresh data before claiming that an
established direct path works without it.

Custom iroh transport plugins are a possible separate experiment. The upstream
custom-transport API is explicitly unstable; a standalone adapter remains the
general integration boundary rather than making that API a natbench requirement.
[Upstream API stability](https://github.com/n0-computer/iroh/blob/v1.3.0/iroh/src/endpoint.rs).

## Delivery order

1. Define the adapter/workload event contract and ship iroh plus a plain QUIC
   reference adapter exchanging the same verified payloads. Demonstrate the
   generic external-executable integration with a public reference implementation.
2. Run matched profile/repetition cases and produce a readable comparison plus
   versioned machine-readable results. Keep packet/log/timeline evidence linked.
   Add the separately built custom implementation locally when its interface is
   available; public documentation uses a generic custom-transport label.
3. Add local iroh relay/traversal cases and test direct, relay, blocked UDP and
   relay interruption. Include only capabilities demonstrated by the adapters.
   Steps 1–3 are the M2 comparison gate; external evaluation remains a launch check.
4. M3 applies composable loss/delay/jitter, MTU/rate limits, rebinding and outages
   to the comparison workload and measures recovery. M4 adds WebRTC/libp2p and
   real TURN where appropriate, plus interoperability within compatible protocols.
