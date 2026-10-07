# Test an application and diagnose a failure

The v0.2.0-alpha.2 preview runs ordinary application executables in controlled
IPv4 networks, supervises their lifecycle, and saves JSON/JUnit, logs, timelines
and optional packet evidence. It is ready for evaluation; the stable M2 launch
still requires outside evaluation. The preview also includes
[matched transport comparisons](ADAPTERS.md) and [real iroh connectivity tests](IROH_CONNECTIVITY.md),
with native example binaries included. The stable v0.1.1 binary supports built-in
scenarios only.

## Install and check the host

Use a Linux machine with namespace and network administration privileges. The
published binaries target glibc-based x86-64 and ARM64 Linux. The fixture executes
your programs with its caller's privileges and shares the host filesystem; choose
an isolated host when evaluating unfamiliar applications. Restricted containers
may fail the active prerequisite check even as root.

Download an archive and `SHA256SUMS` from the
[preview release](https://github.com/0xprames/natbench/releases/tag/v0.2.0-alpha.2).
For the x86-64 archive, from the download directory:

```sh
grep ' natbench-0.2.0-alpha.2-x86_64-unknown-linux-gnu.tar.gz$' SHA256SUMS > selected.sha256
test "$(wc -l < selected.sha256)" -eq 1
sha256sum --check selected.sha256
tar -xzf natbench-0.2.0-alpha.2-x86_64-unknown-linux-gnu.tar.gz
cd natbench-0.2.0-alpha.2-x86_64-unknown-linux-gnu
sudo apt-get update
sudo apt-get install -y iproute2 nftables conntrack tcpdump python3
sudo ./natbench doctor --probe
```

For ARM64, substitute `aarch64-unknown-linux-gnu` in those filenames. Checksums
verify download integrity; they do not independently authenticate the release.
For a source build, clone the repository, run `cargo build --locked`, and use
`./target/debug/natbench` in place of `./natbench` below.

## Reproduce a delivery failure

The included Go example uses only Go's standard library. Install Go separately
(the repository tests Go 1.24), then compile it from the extracted archive or
repository root:

```sh
go build -o examples/udp-echo/udp-echo examples/udp-echo/main.go
sudo ./natbench test examples/udp-echo/failure.json --capture --artifacts ./failure-001
```

This command intentionally exits **1**. The server declares `READY`; the client
must exit 0 and print `PASS first`, but router A drops forwarded UDP. The client
times out and its exit/stdout assertions fail. The report calls this an
`assertion_failed`, distinguishing a failed requirement from a broken fixture.

Read `failure-001/report.json` for the failed assertions,
`failure-001/case-000/timeline.jsonl` for readiness and execution order, and
`failure-001/case-000/client-a-1.stderr.log` for the application's timeout.
Then inspect the same traffic at two locations:

```sh
sudo tcpdump -nn -r failure-001/case-000/a.pcap 'udp port 9999'
sudo tcpdump -nn -r failure-001/case-000/wan.pcap 'udp port 9999'
```

The client's outgoing request is visible locally; no matching UDP reaches the
WAN. The server was ready before the request. In this fixture, those observations
agree with the declared forwarding block. An empty capture by itself cannot
establish a general network diagnosis: inspect capture completeness, drop counts,
truncation and budget exhaustion in `capture.json`.

## Turn the behavior into a regression check

`fixed.json` changes only router A's profile to `preserve`; the application and
delivery assertions are identical. It demonstrates the network condition that
permits delivery, rather than adding UDP fallback to the application.

```sh
sudo ./natbench repeat examples/udp-echo/fixed.json --runs 3 --capture --artifacts ./fixed-001
sudo tcpdump -nn -r fixed-001/run-001/case-000/wan.pcap 'udp port 9999'
```

All three attempts must pass for exit 0. WAN captures now show requests and
replies. `summary.json` and `junit.xml` contain aggregate verdicts and case timing
distributions. Each `run-NNN` retains its own report and evidence. Choose a new
artifact directory for every command; natbench refuses to overwrite a previous run.
Repeated passes describe these attempts on this host, not a reliability guarantee.

The archive also includes `scripts/check-application-demo.sh`. Running
`sudo scripts/check-application-demo.sh ./natbench ./demo-001` verifies the expected
failure, packet evidence, and two corrected runs together (requires Python 3).
The repository runs this same check in CI and against extracted release archives.

## Add your application and keep CI evidence

Copy a scenario next to your application build and change its process `argv`,
roles, readiness and assertions. Relative executable paths and working directories
resolve from the scenario directory. Use argv arrays; commands execute directly.
Have the application discover/connect normally and assert fresh application data,
not merely a handshake. Start with one fast case and add an asymmetric profile
or service outage after the passing case works.

The [copyable GitHub Actions workflow](../examples/github-actions/application.yml)
installs a pinned preview binary, builds the example, checks kernel capabilities,
requires delivery across three attempts, and uploads available evidence even on
failure. Copy it to `.github/workflows/natbench.yml`. For another repository, add
your executable and scenario, then replace the application build and scenario path.
The ordinary command exit status controls CI; avoid masking a failed regression.

Packet files start with mode 0600. The workflow changes artifact ownership to the
runner before upload. Publish evidence only when its application data, logs and
addresses are appropriate to share. To evaluate this preview, follow the
[evaluation checklist](EVALUATION.md). Format details are in
[application scenarios](APPLICATIONS.md), [repetitions](REPETITIONS.md),
[packet capture](PACKETS.md) and [schema compatibility](SCHEMAS.md).
