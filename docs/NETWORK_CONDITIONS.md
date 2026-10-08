# Declare delay and loss in comparisons and application scenarios

The v0.3.0-alpha.1 preview includes comparison input **2** and application
input **4**. Existing comparison input 1 and application inputs 2/3 retain their
behavior. The older alpha.2 predates these controls. See [native-bundle usage](TRANSPORT_QUICKSTART.md).

```sh
cargo build --locked
cargo build --release --locked --manifest-path examples/transports/Cargo.toml
sudo ./target/debug/natbench compare examples/transports/conditions.json --runs 3 --capture --artifacts ./conditions-001
```

The example uses real iroh/Quinn/TCP adapters with an unimpaired reference, upload-only
and download-only delay, and combined delay/random loss. All adapters receive the
same requested workload and network condition; their loss schedules are not identical.
The [verifier](../scripts/check-network-comparison.sh) additionally checks total
loss in each direction, kernel state/counters and packet evidence. Linux and native
x86-64/ARM64 archive CI run it against the actual adapters.

## Matched direct comparisons

Set `schema_version: 2` and require `network` on **every** case, including the
unimpaired reference. The rest of the adapter/workload contract is unchanged:

```json
"network": {
  "client_to_server": {"delay_ms": 15, "loss_percent": 1},
  "server_to_client": {"delay_ms": 25, "loss_percent": 0}
}
```

Client A sends through router A to the WAN receiver. Client-to-server controls
apply to router A's `wan` egress; server-to-client controls apply to the WAN bridge
port `a` toward router A. Router B remains outside this comparison's application
path. Each fresh fixture installs its controls before any application starts.
They remain active until teardown, including during handshake and ARP resolution.

`delay_ms` is an integer from 0 to 1000 per directed link. `loss_percent` is a
finite number from 0 to 100, using independent random loss at the qdisc. Both
fields are required; unknown fields are rejected. Two zeros preserve the existing
qdisc without adding netem queue overhead. Nonzero conditions install a netem root
qdisc with a fixed limit of 4096 queued packets. All traffic on that egress is
subject to the controls, including ARP and transport control packets. Finite queue
limits can also cause drops; counters do not identify every drop's cause.

The report adds `case_conditions` to comparison report 1 and prints the declared
values alongside each cohort. This is additive output metadata; older report 1
files can omit it. Raw inputs and per-attempt generated scenarios keep the same
requested controls. Rerun the original comparison into a new artifact directory
to allocate fresh identities/bootstrap files; the generated per-attempt scenario
is retained as execution evidence.

## Ordinary applications and repeat

Application input 4 extends input 3's lifecycle steps with a required per-case
`network` object. It uses named fixture egress links so programs in any role or
language can declare conditions. For example, add this to a version 4 case:

```json
"network": {
  "links": [
    {"egress": "router_a_wan", "conditions": {"delay_ms": 15, "loss_percent": 1}},
    {"egress": "wan_router_a", "conditions": {"delay_ms": 25, "loss_percent": 0}}
  ]
}
```

A case has 1–8 unique egress links. Unlisted links retain their defaults. Both
`test` and `repeat` accept input 4, including optional packet capture. Readiness,
process supervision, waits, assertions and cancellation keep their existing
semantics. A lifecycle `delay` step waits in the controller; it does not change
network propagation delay.

| Egress name | Namespace/device | Packet direction |
| --- | --- | --- |
| `client_a` | a / eth0 | A to router A |
| `router_a_lan` | ra / lan | Router A to A |
| `router_a_wan` | ra / wan | Router A to WAN |
| `wan_router_a` | wan / a | WAN to router A |
| `client_b` | b / eth0 | B to router B |
| `router_b_lan` | rb / lan | Router B to B |
| `router_b_wan` | rb / wan | Router B to WAN |
| `wan_router_b` | wan / b | WAN to router B |

Controls on different links compose along the packet path. These names describe
the fixed five-role fixture, not arbitrary host interfaces or nested topologies.
Comparison input 1 and application inputs 2/3 reject a `network` field even if
its value is null; opt into the new input version explicitly.

## Kernel evidence and failure behavior

New inputs require `tc` from iproute2 on root's PATH before creating artifacts or
namespaces. Nonzero conditions also require kernel netem support. `doctor --probe`
reports netem as an optional general capability; it becomes necessary when these
cases request it. A missing tool is a preflight error. A kernel configuration or
evidence error becomes a fixture error, not a transport success/failure sample.

Each shaped application case retains `network.json` with schema 1, kind
`network_conditions`. It records tc version, requested conditions, role/device
bindings, queue limits, configuration arguments and raw `tc -j -s qdisc` snapshots
before configuration, after configuration and before namespace teardown. Kernel
options and counters keep tc's version-specific representation. The final snapshot
also runs on setup failure, process failure and SIGINT/SIGTERM while the fixture
still owns its namespaces. Partial setup records which links were installed.

`configured` means every requested control was installed or explicitly left at
zero. `complete` means final evidence was collected without errors and the final
qdisc configuration still matches the configured snapshot. Counters can change;
changed final settings fail the fixture. This checks the snapshots, not continuous
attestation of an application's privileged behavior. `interrupted` remains explicit;
a complete evidence file does not turn an interrupted or failed test into a pass.
Suite/compare JSON and exit status remain authoritative. SIGKILL or an I/O failure
can leave only a partial evidence checkpoint.

Declared loss is a probability, not a promise that this short sample loses an
exact percentage. Kernel PRNG seeds are not configured; repeated runs do not have
identical loss schedules. Timer granularity, scheduling, queue limits and packet
segmentation/offload can affect observed behavior and counter units. These are
controlled synthetic conditions, not an exact reproduction of a user's network.
The [netem manual](https://man7.org/linux/man-pages/man8/tc-netem.8.html) documents
its timing, queueing and placement limits. Successful-attempt latency/goodput
remains conditional on delivery; failed attempts retain counts without fabricated
performance measurements.

[Application input 5](TRANSPORT_RECOVERY.md) adds scheduled link changes, outages
and fresh-data recovery experiments. Jitter, rate/MTU controls, relay restart and
rebinding remain follow-up work in the [delivery queue](DELIVERY_QUEUE.md).
