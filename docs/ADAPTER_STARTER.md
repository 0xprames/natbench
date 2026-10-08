# Build an independent Rust transport adapter

The v0.3.0-alpha.1 preview includes a copyable Rust project, `natbench conform`, and
a bundled starter executable. The older v0.2.0-alpha.2 predates these additions.
The starter uses plain Linux TCP without TLS or endpoint authentication. Replace
its transport module with your library while retaining the executable contract.

## Create a separate project

From the natbench checkout or an extracted preview bundle:

```sh
scripts/new-adapter.sh ../custom-transport-adapter custom-transport-adapter
cargo build --release --locked --manifest-path ../custom-transport-adapter/Cargo.toml
sudo ./natbench conform ../custom-transport-adapter/adapter.json --capture --artifacts ./conformance-001
```

For a source-built controller use `./target/debug/natbench`. The destination must
be new and its parent must exist. The generator copies adapter source, its locked
dependencies, license and the small shared protocol crate into the new project;
it does not link natbench's core or reference transport libraries. The generated
project builds independently without a path dependency on the original checkout.
It includes a [copyable CI workflow](../examples/github-actions/adapter.yml);
pin its controller source ref to an evaluated release/commit.
The package name is an optional safe lowercase identifier; default is
`custom-transport-adapter`. Source builds require Rust 1.91 or newer/current stable.

The [starter source](../examples/adapter-starter/src/main.rs) validates requests,
publishes a private atomic peer file, flushes readiness/terminal JSONL events,
enforces the entire client deadline, and verifies workload metrics. The
[transport module](../examples/adapter-starter/src/transport.rs) owns sockets,
bounded stream framing, connection lifetime, receiver byte verification and
socket/path evidence. Replace `connect`, `serve`, `Connection` and `settings`
with your actual library API. Record your engine/version, stream/framing behavior,
security and effective settings; keep ephemeral addresses in peer/path evidence.
The initial backend enables `TCP_NODELAY` at both endpoints and records read-back
socket settings. Its source fingerprint covers the adapter manifest, lockfile,
build script and adapter sources; record additional library build information.

Keep attempt ID, sequence and exact payload verification before responding.
The bulk acknowledgement must follow complete receiver verification. Payload
construction precedes message/bulk timing; response verification ends it. First
data includes creating/dialing the client endpoint and verifying an echo, with
server readiness/peer-file loading earlier. See [measurement semantics](ADAPTERS.md).

## Check one adapter

[Adapter config 1](../schemas/transport-adapter-config-v1.schema.json), kind
`transport_adapter_config`, contains one ordinary executable declaration:

```json
{
  "schema_version": 1,
  "kind": "transport_adapter_config",
  "adapter": {
    "name": "custom-transport",
    "argv": ["./target/release/custom-transport-adapter"]
  }
}
```

`cwd`/`env` have the same semantics as [comparison adapters](ADAPTERS.md).
The controller appends `--request ABSOLUTE_PATH`. It checks one adapter with fresh
fixtures and identities, using a NATed IPv4 client A and WAN receiver. It requires
Linux/root, iproute2 including `tc`, nftables, kernel netem, and tcpdump for capture.
Programs use the fixture caller's privileges and share its filesystem, as ordinary
application scenarios do. `--runs` accepts 1–100 repetitions, default 1.

| Cases | Payload / warm-up / measured / bulk | Expected result |
| --- | --- | --- |
| Minimum behind preserve and random NAT | 1 byte / 0 / 1 / 1 KiB | Verified delivery |
| Typical behind preserve and random NAT | 128 bytes / 2 / 10 / 1 MiB | Verified delivery |
| Maximum behind preserve and random NAT | 64 KiB / 100 / 1000 / 16 MiB | Verified delivery |
| Total loss client→server or server→client | Minimum workload; 2-second client deadline | Explicit transport failure without completed metrics |

Normal workloads have a 10-second deadline; maximum workloads have 60 seconds.
The checker reuses request/bootstrap/JSONL validation, readiness and process
supervision, identity/version/settings agreement, terminal-event/exit consistency,
bounded parsing, measurement validation and cleanup. All declared workload bounds
must be supported for a complete pass. Failure under total loss is an expected
conformance result; its actual transport-failed outcome remains in raw evidence.

Malformed/stale/missing events, invalid metrics, incompatible identities, unexpected
process exits and fixture errors fail conformance. Unsupported capability keeps
its distinct outcome and nonzero exit 3, including JUnit skips; it cannot become a
passing check. The starter's `--unsupported` flag demonstrates that path and does
not run the workload or pass conformance. SIGINT/SIGTERM retain the active evidence
and future planned verdicts; incomplete or interrupted checks cannot pass.

The checker validates reported contract/lifecycle claims and runs the real fixture.
It cannot attest to hidden implementation behavior or prove that an arbitrary
adapter honestly checks bytes. The starter's receiver/framing tests reject stale,
reordered, corrupted, oversized and truncated frames; its real verifier exercises
actual TCP delivery and failure. Your adapter must perform equivalent byte checks.
Conformance does not add discovery/traversal or relay behavior to a bare transport.

## Evidence and CI

[Conformance report 1](../schemas/transport-adapter-conformance-report-v1.schema.json)
has kind `transport_adapter_conformance_report`, with every expected/observed
verdict, completion/interruption and exit. Artifacts include the original private
`adapter.json`, primary `report.json`/`junit.xml`, and `runs/` with the
[generated execution plan](../schemas/transport-adapter-conformance-plan-v1.schema.json),
[raw transport attempts](../schemas/transport-adapter-conformance-runs-v1.schema.json),
requests/peer files, logs, timelines, kernel conditions and optional bounded PCAPs.
The plan/raw report have their own kinds and are execution evidence; they cannot
be supplied to `compare`/`assess` as a transport comparison.

Publish the primary conformance JUnit for the checker verdict. Nested suite/raw
JUnit retains actual transport outcomes, including expected total-loss failures.
Require exit 0, complete true and interrupted false. Exit 1 is a conformance mismatch,
2 is input/contract/fixture/artifact error, 3 is unsupported/incomplete, and 130 is
interruption. Expected negative outcomes do not erase their raw failure evidence.

The [starter verifier](../scripts/check-adapter-starter.sh) checks all eight real
cases and eight explicit unsupported cases. Linux and native x86-64/ARM64 CI also
generate/build a separate project and run its conformance. Native archive checks
exercise the bundled executable without Rust on PATH. An external implementation
can stay in its own repository; add its argv to a matched comparison after these
checks pass. This starter shares direct-stream application semantics, not a common
wire protocol with another stack.
