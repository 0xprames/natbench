# Compare transports, keep evidence and test a custom adapter

Use the **v0.3.0-alpha.1 evaluation preview** on glibc-based x86-64 or ARM64 Linux.
It includes the controller and four separately built example executables; the
comparison, gates and recovery checks need no Rust installation. Linux/root,
network namespaces, nftables, iproute2 including `tc`, kernel netem and conntrack
are required; capture needs tcpdump and verification scripts need Python 3.

## Install the native bundle

For x86-64, in a new working directory:

```sh
version=0.3.0-alpha.1
archive="natbench-${version}-x86_64-unknown-linux-gnu.tar.gz"
base="https://github.com/0xprames/natbench/releases/download/v${version}"
curl -fL --retry 3 "$base/$archive" -o "$archive"
curl -fL --retry 3 "$base/SHA256SUMS" -o SHA256SUMS
awk -v archive="$archive" '$2 == archive {print}' SHA256SUMS > selected.sha256
test "$(wc -l < selected.sha256)" -eq 1
sha256sum --check selected.sha256
tar -xzf "$archive"
cd "${archive%.tar.gz}"
sudo apt-get update
sudo apt-get install -y iproute2 nftables conntrack tcpdump python3
sudo ./natbench doctor --probe
```

Substitute `aarch64-unknown-linux-gnu` for ARM64. Checksums verify download integrity,
not independent authenticity. A restricted container may lack the required kernel
privileges. Programs share the filesystem and caller privileges. Use a new artifact
directory for each command; existing evidence is never overwritten.

## Compare and assess the bundled implementations

```sh
sudo ./natbench compare examples/transports/direct.json --runs 3 --capture --artifacts ./comparison-001
sudo chown -R "$(id -u):$(id -g)" comparison-001
./natbench assess comparison-001/report.json --policy examples/transports/regression-policy.json --artifacts ./assessment-001
```

The comparison rotates iroh, Quinn and plain TCP through the same fresh verified
message/bulk workloads behind preserve/random NAT. Inspect counts before comparing
conditional first-data/RTT/goodput samples. Plain TCP has no TLS/authentication;
stream/framing and security differ across the implementations. These are direct
client-to-WAN workloads, not peer traversal or relay rankings.

`assess` reads saved reports without root or networking tools. This example policy
requires three successful attempts per cohort, full delivery, p95 RTT ≤100 ms and
minimum verified goodput ≥1 MiB/s. Those are editable example requirements, not
universal performance claims. Exit 1 means a requirement failed; exit 2 means
invalid evidence/configuration, exit 3 incomplete/unsupported evidence, and 130
interruption. Read the assessment's JSON/JUnit and coverage before changing a bound.
[Saved-baseline policies](REGRESSION_GATES.md) also require compatible machine,
workload, conditions and settings, with explicit build-version allowances.

For matched delay/loss conditions:

```sh
sudo ./natbench compare examples/transports/conditions.json --runs 3 --capture --artifacts ./conditions-001
```

The shipped regression policy names the *direct* cohorts; use a policy naming every
shaped cohort with appropriate bounds to gate this different experiment. Requests
and kernel settings/counters remain beside each attempt's logs, timeline and PCAPs.

## Check outage recovery and iroh connectivity

```sh
sudo scripts/check-transport-recovery.sh ./natbench ./bin/natbench-transport-recovery ./recovery-001
sudo scripts/check-iroh-connectivity.sh ./natbench ./bin/natbench-iroh-connectivity ./iroh-001
```

Recovery checks nine real upload/download/bidirectional outages and three expected
persistent-outage failures. Successful cases send and verify a **new sequence after
restoration** on the existing connection. The interval includes marker handling,
retransmission and scheduling; it is not pure transport latency. Persistent failures
retain their deadline/phase without a recovery metric. Inspect
`recovery-001/verified-recoveries.json` and the raw runs/markers/kernel drops.

The separate iroh verifier exercises two NATed peers, local relay/address discovery,
forced/automatic paths, blocked UDP and relay shutdown. A successful expected
negative scenario proves its failure assertion; it does not mean data was delivered.
See [recovery semantics](TRANSPORT_RECOVERY.md) and [iroh scope](IROH_CONNECTIVITY.md).

## Bring a custom Rust transport

First try the bundled starter against the contract:

```sh
sudo ./natbench conform examples/adapter-starter/adapter.json --capture --artifacts ./starter-001
```

It runs minimum/typical/maximum workloads and directional total-loss negatives.
To build your independent adapter, install Rust 1.91 or newer/current stable:

```sh
scripts/new-adapter.sh ../my-transport-adapter my-transport-adapter
cargo build --release --locked --manifest-path ../my-transport-adapter/Cargo.toml
sudo ./natbench conform ../my-transport-adapter/adapter.json --capture --artifacts ./custom-conformance-001
```

Replace `src/transport.rs` with your real backend, record its settings/security and
retain exact receiver byte verification. The initial generated backend is plain TCP.
The generated project contains its protocol source and locked dependencies, plus a
CI workflow pinned to this preview. It builds independently of the natbench checkout.
[Conformance](ADAPTER_STARTER.md) validates contract claims and fixture behavior; the
adapter remains responsible for honest byte verification.

To compare your generated adapter against bundled iroh and Quinn, create a config
from the same direct workloads. Resolve executables before moving the config:

```sh
python3 - <<'PY'
import json,pathlib
config=json.loads(pathlib.Path('examples/transports/direct.json').read_text())
for adapter in config['adapters']:
    adapter['argv'][0]=str(pathlib.Path('bin/natbench-transport-adapters').resolve())
custom=config['adapters'][-1]
assert custom['name']=='tcp'
custom['name']='custom-transport'
custom['argv']=[str(pathlib.Path('../my-transport-adapter/target/release/my-transport-adapter').resolve())]
pathlib.Path('custom-comparison.json').write_text(json.dumps(config,indent=2)+'\n')
PY
sudo ./natbench compare custom-comparison.json --runs 3 --capture --artifacts ./custom-comparison-001
```

Use a regression policy naming these custom cohorts to gate their results.
Adapters can use any language and stay separately built/private. They do not gain
traversal/discovery support from the fixture. [The contract](ADAPTERS.md) explains
capabilities, limits and measurement semantics.

## Keep it in CI and send evaluation findings

Copy the [native transport workflow](../examples/github-actions/transports.yml) or
the [independent adapter workflow](../examples/github-actions/adapter.yml). Both
pin this preview and preserve available evidence after failures.

Record your host/version, integration goal, actual/expected outcome and the largest
obstacle in [an evaluation issue](https://github.com/0xprames/natbench/issues/new).
Share suitable public CI evidence or a summary without sensitive logs/packet data.
The stable v0.2.0 outside-evaluation gate remains open. This preview also leaves
jitter/rate/MTU, relay restart, NAT rebinding, nested/IPv6 networks and broader
transport stacks for later slices; it does not complete M3.
