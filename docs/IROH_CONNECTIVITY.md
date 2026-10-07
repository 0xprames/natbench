# Test real iroh traversal and relay delivery

The v0.2.0-alpha.2 preview includes `natbench-iroh-connectivity`, a standalone
Rust example using pinned iroh and iroh-relay 1.3.0. It runs as ordinary application
programs; natbench's core does not link either library. The older v0.2.0-alpha.1
predates this example. The Linux fixture needs namespace privileges, iproute2,
nftables, tcpdump and Python 3.

From the [verified native archive](GETTING_STARTED.md#install-and-check-the-host),
run the bundled executable without installing Rust:

```sh
sudo scripts/check-iroh-connectivity.sh ./natbench ./bin/natbench-iroh-connectivity ./iroh-001
```

For a repository checkout, build the examples separately:

```sh
cargo build --locked
cargo build --release --locked --manifest-path examples/transports/Cargo.toml
sudo scripts/check-iroh-connectivity.sh ./target/debug/natbench ./examples/transports/target/release/natbench-iroh-connectivity ./iroh-001
```

The verifier creates fresh requests and eight application scenarios, executes them
serially and checks the actual JSONL events. Each case owns a new fixture, endpoint
identities, certificate and bootstrap/control files. Keep the artifact directory
new. Native archives include both the executable and separately buildable source.

## Placement and bootstrap

Peer A lives at 10.1.0.2 behind router A (WAN 198.18.0.10); peer B lives at
10.2.0.2 behind router B (WAN 198.18.0.20). Both routers drop unsolicited input.
The WAN hosts a real HTTPS iroh relay at `https://198.18.0.1:8443/`, its HTTP
probe service at port 8080, and QUIC address discovery at UDP port 7842. A fresh
self-signed certificate is explicitly trusted by the peers; TLS verification
remains enabled. The relay accepts the fixture's peers without additional access
restrictions. It is an isolated test service, not a production relay deployment.

The controller supplies **only peer B's identity and relay URL** in a declared
file after B registers with the relay. It does not supply public UDP mappings,
punch sockets, or run STUN on the peers' behalf. Iroh performs QUIC address discovery
and its own discovery/hole punching over the local relay. Address-lookup services
and public relays are disabled. This exercises traversal with explicit bootstrap;
it does not test internet DNS/identity discovery.

Automatic policy enables IPv4 UDP and the local relay. Forced relay policy removes
IP transports on **both** endpoints. Blocking forwarded UDP leaves the relay's TCP
connection available. The random profile is kernel `masquerade fully-random`, not
a guaranteed symmetric-NAT model; its selected path is observed, not predetermined. The any-path case observes path
selection for up to two seconds before accepting a relay path; it can accept direct
as soon as that path is selected. This bounded observation is not a prediction of
eventual connectivity on another host.

## Cases and assertions

| Case | Required evidence |
| --- | --- |
| Automatic, preserve/preserve | Direct path selected and complete verified workload |
| Automatic, random/random | Verified delivery; record whichever path iroh selects |
| Automatic, blocked/blocked | Relay selected and complete verified workload |
| Automatic, preserve/blocked | Relay selected and complete verified workload |
| Forced relay, preserve/preserve | Relay selected; no IP paths; complete verified workload |
| Direct connection, relay stopped | Direct selected before stop; fresh messages and bulk acknowledged afterward on the same connection |
| Forced relay, relay stopped | Delivery before stop; no fresh acknowledgement afterward; application exchange failure |
| Blocked UDP, relay stopped | Delivery before stop; no fresh acknowledgement afterward; application exchange failure |

The workload uses the same attempt-bound frames and receiver byte verification as
the direct-stream adapters: first message, two warm-up messages, ten messages and a
256 KiB bulk transfer. Every message opens a bidirectional stream on one connection.
The receiver verifies the attempt ID, sequence and payload before echoing; bulk
gets a small acknowledgement only after full verification. Client and server logs
must agree on every completed sequence. These are connectivity assertions, not a
performance ranking. Per-exchange times include verification and any path changes;
first data may arrive over relay before a direct path is selected.

For interruption cases the client emits `before_relay_stop` only after verified
initial delivery and the required selected path. It pauses further application
exchanges. The scenario stops the relay process group, then a short controller
program atomically publishes an attempt-bound resume signal. Only then does the
client submit fresh frames. Direct success uses the existing connection without
redialing. Relay-dependent delivery fails with phase `exchange_after_relay_stop`
within a two-second per-exchange deadline; no complete workload or bulk metric is
invented. This demonstrates an observed outage, not reconnection after restart.

## Inspect the evidence

`summary.json` distinguishes `verified_delivery` and `expected_relay_outage`,
records the selected path, policy, verified counts and adapter source fingerprint,
and links each case's evidence directory. `complete: true` means every expected
behavior and evidence check passed. A nonzero verifier exit is a failed evaluation.
The summary is produced by the verifier, independently from core comparison reports.

Each case keeps its input requests and scenario, bootstrap, public certificate,
JSON/JUnit suite report, lifecycle timeline, stdout/stderr and bounded five-role
PCAPs. JSONL events have schema 1, kind `iroh_connectivity_event`, attempt identity,
policy, pinned library versions, compiler/build metadata and a source fingerprint.
They report connection establishment, verified/received sequences, path snapshots,
relay-stop gates, completion or the failure phase. The snapshot records every open
path's ID, kind, selected flag and addresses from iroh's public `Connection::paths`
API. Only a path with `selected: true` describes the selected transmission path.
An open relay path can remain in a snapshot after its server was stopped; delivery
assertions and the runner's stop timeline establish what still works.

`request` schema 1 is specific to this example and rejects unknown fields. Absolute
paths, distinct artifact files, bounded workload/deadlines, attempt IDs and policy
consistency are validated before creating an endpoint. Server/relay readiness is
supervised by the application runner; the client has a whole-operation deadline.
PCAP packet limits can truncate traffic; consult each capture manifest. Do not
infer a failure location or packet absence from an exhausted capture.

The verifier also cancels a live relay/peer fixture with SIGTERM, checking exit 130,
partial reports, retained packet evidence and namespace cleanup.

## Scope relative to compare

Use `natbench compare` for matched direct-stream iroh/Quinn/custom adapter
measurements. Use these application scenarios for real iroh connectivity coverage.
They are separate experiments: this example does not add traversal to bare Quinn,
produce cross-stack relay performance rankings, or introduce a new generic stack
adapter contract. Relay restart/recovery, composed impairments, rebinding, IPv6,
other connectivity stacks and real-network observations remain further work.

Upstream APIs used: [relay server](https://github.com/n0-computer/iroh/blob/v1.3.0/iroh-relay/src/server.rs),
[endpoint configuration](https://github.com/n0-computer/iroh/blob/v1.3.0/iroh/src/endpoint.rs),
and [connection paths](https://github.com/n0-computer/iroh/blob/v1.3.0/iroh/src/endpoint/connection.rs).
