# Verify delivery after a scheduled outage

Current development source adds application input **5** and a separately built
`natbench-transport-recovery` example. The published v0.2.0-alpha.2 predates it.

```sh
cargo build --locked
cargo build --release --locked --manifest-path examples/transports/Cargo.toml
sudo scripts/check-transport-recovery.sh ./target/debug/natbench ./examples/transports/target/release/natbench-transport-recovery ./recovery-001
```

The verifier runs iroh, Quinn and plain TCP through real Linux namespaces, with
upload, download and bidirectional one-second outages behind preserve/random NAT.
Nine cases verify recovery; three persistent-outage cases require the client to
reach its deadline and fail. An expected negative case passes the *scenario*
assertion while retaining the client's failed recovery event. It contributes no
recovery timing. JSON/JUnit, logs, kernel drop counters and PCAPs are preserved.

To run or repeat the example directly:

```sh
sudo ./target/debug/natbench test examples/transports/recovery.json --capture --artifacts ./recovery-002
sudo ./target/debug/natbench repeat examples/transports/recovery.json --runs 3 --capture --artifacts ./recovery-repeat-001
```

## Schedule a change in an application scenario

Input 5 retains input 4's required initial `network` and lifecycle steps. It adds:

```json
{"action":"network_set","id":"outage","marker":"outage.json",
 "network":{"links":[{"egress":"router_a_wan",
 "conditions":{"delay_ms":0,"loss_percent":100}}]}}
```

The action changes only named links already declared in the initial network.
Other links keep their current settings. Conditions use the same bounded delay
and loss fields as input 4. A subsequent zero condition removes natbench's netem
qdisc. A `delay` between actions schedules a hold interval; it is not packet delay.
Changing multiple links is sequential, not atomic. Replacement/removal can discard
queued packets. This example starts without a queue and verifies actual drops.

Event IDs and marker filenames must be unique within a case. IDs contain 1–64
ASCII letters, digits, underscores or hyphens. Markers are safe local `.json`
filenames, at most 64 characters; hidden/path/double-dot names and controller
network/capture filenames are rejected. A marker collision fails the fixture.

Every child receives `NATBENCH_CASE_ARTIFACTS` (absolute case directory) and
`NATBENCH_RUN_ID` (fresh 32-character hexadecimal identity). Input 5 reserves
these environment variables. Repetitions receive new directories and identities.
The controller publishes a private atomic marker only after every requested link
change and kernel snapshot succeeds. Applications should validate the marker's
kind, version, identity, completion and timestamps before trusting it.

## What the recovery interval measures

The client establishes one connection and verifies sequence 0 before the outage.
After the outage marker, it sends sequence 1 and waits on that same connection.
The controller restores the network after the declared hold interval. The client
waits for the restoration marker and then sends fresh sequence **2**, checks its
exact response, and sends fresh bulk sequence **3** with a receiver verification
acknowledgement. Payloads embed the per-case identity and sequence.

`event_to_fresh_delivery_seconds` runs from the controller's restoration-completed
`CLOCK_MONOTONIC` timestamp to receipt and verification of sequence 2. Both processes
share the host clock because the fixture creates network namespaces, not time
namespaces. The interval includes marker publication/polling, retransmission and
application scheduling. Sequence 1 was queued during the outage and is excluded.
This is an end-to-end recovery observation, not a pure transport latency ranking.

The example records before/after transport path evidence, verified message/bulk
byte counts, raw timestamps, one connection attempt and
`existing_connection_survived: true` only after successful fresh delivery. It never
reconnects. Persistent loss emits `failed` with a phase/deadline and no interval.
The workload and security/framing settings match the existing reference code;
plain TCP has no TLS/authentication. Different transport security and framing
semantics still apply. These experiments are separate from direct performance
comparison cohorts and offline `assess` policies.

## Evidence and limits

Input 5 writes network evidence **2**. It retains initial requested/configured
state, latest current/configured state, final counters, and every transition's
before/after snapshots, argv, completion/error and monotonic timestamps. Successful
markers use transition **1**. A partial multi-link change remains incomplete and
publishes no successful marker. Kernel drift, configuration/snapshot/marker errors
are fixture failures (exit 2); cancellation keeps evidence and returns 130.

Ordinary application logs contain example-specific recovery events **1**, alongside
the server's standard transport-ready event. The verifier writes
`verified-recoveries.json` as a convenience summary; raw suite results, markers and
logs remain authoritative. A custom application can implement its own recovery
policy using the same input 5 actions and marker protocol.

Relay restart, NAT rebinding, reconnecting recovery policies and separate recovery
regression gates remain later slices. This fixture currently exercises a direct
IPv4 path with a scheduled link outage. See [the delivery queue](DELIVERY_QUEUE.md).
