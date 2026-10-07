# Run matched transport comparisons

The v0.2.0-alpha.2 preview includes `natbench compare`, independent iroh 1.3.0
and Quinn 0.11.12 adapters, and their native binaries under `bin/`. The core uses
the small shared request/event contract; it does not link iroh. The older
v0.2.0-alpha.1 release predates this command.

## Use the native bundle

[Download and verify the preview](GETTING_STARTED.md#install-and-check-the-host),
install the Linux fixture prerequisites and run from the extracted directory:

```sh
sudo ./natbench compare examples/transports/direct.json --runs 4 --capture --artifacts ./comparison-001
```

No Rust toolchain is needed. The bundled configuration resolves its adapter
executables from `bin/`; `comparison-001/report.json`, `junit.xml` and per-attempt
evidence retain outcomes and measurements. Choose a new artifact directory each time.
The [copyable transport workflow](../examples/github-actions/transports.yml) downloads
the pinned bundle, runs this comparison and the real iroh connectivity verifier,
and uploads available evidence on failure.

## Build from source

Build the core and adapters separately with a current stable Rust toolchain
(iroh requires at least Rust 1.91). The repository's configuration resolves the
adapters from their Cargo build directory:

```sh
cargo build --locked
cargo build --release --locked --manifest-path examples/transports/Cargo.toml
sudo ./target/debug/natbench compare examples/transports/direct.json --runs 4 --capture --artifacts ./comparison-001
```

The example compares reliable request/response streams and a verified 1 MiB bulk
transfer behind preserve and random NATs. The receiver is on the WAN, at
198.18.0.1:9443; the client is behind router A. Router B has no application role
in this experiment. There is no peer hole punching, discovery or relay in this
cohort. Peer information is exchanged through a declared controller-owned file.
Both implementations create fresh endpoints and use their own sockets/protocol.

## What the numbers mean

- **First verified data:** client monotonic time from before creating/dialing its
  endpoint to verifying the first echoed application frame. Server readiness and
  loading the provided peer information precede this interval.
- **Message RTT:** sequential request/response samples after the declared warm-up.
  Each reference exchange opens a new bidirectional stream on the same connection.
  Payload construction precedes timing; response verification finishes the interval.
- **Verified bulk goodput:** bulk payload bytes divided by the time to send a stream,
  verify its entire contents at the receiver, and receive/verify a small acknowledgement.
  Protocol framing is excluded from the byte numerator. This includes receiver
  verification and acknowledgement, rather than measuring a send-buffer write.

Every frame carries a fresh attempt ID and sequence. Both reference implementations
verify the same deterministic payload bytes before responding. The bulk receiver
acknowledges only verified data. Warm-up samples are excluded from message RTT.
Whole-fixture duration remains in the nested suite evidence; it is not transport
latency. A failed or unsupported attempt has no completed performance measurement.

Summary first-data/goodput distributions contain one sample per successful attempt.
Message RTT pools the raw measured message samples from successful attempts in
that cohort; messages within an attempt are correlated. Sample counts are explicit.
P95 uses nearest rank, becoming the maximum with fewer than 20 samples. These
summaries do not provide confidence intervals or reliability guarantees.

Iroh uses noq with mutual endpoint identity authentication; the Quinn reference
uses a pinned server certificate without client certificate authentication. Both
use ring, library transport defaults, reliable streams and IPv4 direct paths.
Versions, compiler, build profile, adapter source/lock fingerprint and settings are saved. This compares these
configured implementations; it does not isolate the overhead of one wrapper over
an identical QUIC engine/security configuration. Iroh's public lookup/relay services
are disabled. No public infrastructure is required.

Use release adapter builds and an appropriate, otherwise quiet host for performance
investigation. Packet collection can affect timings; its settings are recorded.
Keep capture and workload settings matched. Synthetic single-host results describe
this setup. Discovery, NAT traversal, relay fallback and changing networks remain
the next cohorts in the [transport comparison plan](TRANSPORT_COMPARISONS.md).

## Input, ordering and evidence

[Comparison input version 1](../schemas/transport-comparison-v1.schema.json)
declares 2–8 named adapters and 1–32 cases. Each case names one router A profile,
a client deadline (1–60000 ms), and the common workload. `--runs` accepts 1–100
repetitions, default 4; the entire matrix is capped at 4096 attempts. Split larger
experiments into explicit jobs. Unknown input fields, invalid bounds and missing
working directories are rejected before artifacts or fixtures are created.

Adapters have an argv array, optional cwd (relative to the comparison input), and
optional environment overrides. The controller appends `--request ABSOLUTE_PATH`.
Executables resolve from cwd or the effective PATH, as in application scenarios.
Programs remain foreground and execute with the fixture caller's privileges.
They share the host filesystem; this is a network fixture, not an application sandbox.

For each case/repetition, adapter order rotates cyclically. Exact position balance
requires a run count divisible by the adapter count. Each attempt owns a fresh
network fixture and new request/bootstrap paths. Input bytes are frozen once;
application binaries, cwd contents and inherited environment remain caller context.
If reported identity/version/settings change within a case/adapter cohort, the
attempt becomes an adapter error and its performance samples are excluded.

Choose a new artifact directory whose parent already exists. It contains:

- `comparison.json`: original validated input bytes.
- `report.json` and `junit.xml`: checkpoints of planned slots, completed outcomes,
  active state, environment and conditional metrics.
- `attempt-NNNN/`: server/client requests, opaque peer bootstrap, generated scenario,
  and `evidence/` with suite JSON/JUnit, process logs and timeline. `--capture[=N]`
  adds the existing bounded per-role PCAPs and manifests.

[Report version 1](../schemas/transport-comparison-report-v1.schema.json) uses kind
`transport_comparison_report`. Attempt indices form a zero-based prefix of `planned`;
`active_attempt` is the next slot, or null. `complete` says every slot has an outcome,
including failed/unsupported/interrupted outcomes. Success also requires exit 0 and
`interrupted: false`. Summary outcome counts plus unreported slots equal requested
runs. Completed measurements exist only for passing attempts and retain raw samples.

| Exit | Meaning |
| --- | --- |
| 0 | Every planned workload completed with verified delivery |
| 1 | At least one transport workload failed |
| 2 | Invalid input, fixture/artifact error or adapter contract violation |
| 3 | Unsupported capability or incomplete coverage |
| 130 | Interrupted comparison |

Precedence is interruption, errors, unsupported/incomplete, then transport failure.
Unsupported entries are skipped in JUnit but keep the CLI nonzero. Later successes
do not erase prior failures. SIGINT/SIGTERM preserve active evidence and future
unreported slots. JSON checkpoints are authoritative if a crash leaves JUnit behind.
SIGKILL/power loss may retain only the last checkpoint and leave fixture resources.

## Implement an external adapter

Wrap your library or executable with this file/JSONL interface; no natbench core
change is needed. Use a generic label such as `custom-transport` in public examples.

1. Read and validate the [request](../schemas/transport-request-v1.schema.json).
   It declares server/client role, fresh `run_id`, IPv4 listen/peer addresses,
   absolute peer-file path, deadline and workload. Version 1 means direct reliable
   stream workloads. Workload bounds are payload 1–65536 bytes, 0–100 warm-ups,
   1–1000 measured messages and bulk 1024–16777216 bytes.
2. The server writes a new, complete [peer bootstrap](../schemas/transport-peer-v1.schema.json)
   before emitting/flushing one JSONL `ready` event, then remains alive until stopped.
   Bootstrap details are implementation-specific; the envelope carries version,
   kind, attempt ID and implementation identity/settings. Use atomic publication
   and private permissions. Keep ephemeral addresses in bootstrap/path evidence,
   rather than changing the shared settings identity between server and client.
3. The client consumes that bootstrap and uses its real transport. Require fresh,
   receiver-verified bytes and an application acknowledgement; do not count a
   handshake, queued write or stale response as delivery. Implement equivalent
   workload semantics and describe any extra application framing in settings.
4. Emit exactly one terminal [event](../schemas/transport-event-v1.schema.json):
   `completed` with all valid metrics and exit 0, `failed` with phase/message and
   exit 1, or `unsupported` with reason and exit 3. A bootstrap-phase failure is an
   infrastructure/contract error. Flush stdout; keep diagnostic prose on stderr.
   The server/client/bootstrap IDs and implementation metadata must agree.

JSONL output allows additive fields within version 1. The runner bounds parsing
to 128 events and 8 MiB per process log, plus 1 MiB per event or bootstrap.
Wrong versions/IDs, multiple or missing terminal events, exit/event disagreement,
incomplete/nonfinite metrics and incompatible readiness become adapter errors.
These parsing limits do not impose a disk quota on application logs. Reference
sources live in [the standalone adapter crate](../examples/transports/) and
[the shared protocol crate](../crates/transport-protocol/).

## Real iroh connectivity coverage

The separately built [iroh connectivity example](IROH_CONNECTIVITY.md) exercises
two NATed peers, a real local relay/address-discovery service, automatic and forced
relay policies, blocked UDP and relay interruption. Run its verifier through
application scenarios. These tests do not change the direct-stream comparison
contract or mix relay samples into its performance summaries.
