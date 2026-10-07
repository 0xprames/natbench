# Packet evidence for application scenarios

The v0.2.0-alpha.1 preview supports `--capture` on `test` and `repeat` for application input
versions 2 and 3. Install `tcpdump` alongside the normal fixture prerequisites.
`doctor` checks for this optional tool; capture explicitly requested without it
fails before creating artifacts or namespaces. Built-in input version 1 does not
support this capture flag.

```sh
go build -o examples/udp-echo/udp-echo examples/udp-echo/main.go
cargo build --locked
sudo ./target/debug/natbench test examples/udp-echo/scenario.json --capture --artifacts ./packets-001
sudo tcpdump -nn -r ./packets-001/case-000/wan.pcap 'udp port 9999'
sudo tcpdump -nn -r ./packets-001/case-001/a.pcap 'udp port 9999'
sudo tcpdump -nn -r ./packets-001/case-001/wan.pcap 'udp port 9999'
```

The working case shows application requests and replies on the WAN. In the blocked
case, the client's outgoing UDP appears in `a.pcap` while the WAN has no matching
UDP. The independent application's timeout is retained in its stderr log. The
example expects that negative application result, so the scenario itself passes.
Use exit/stdout assertions for the behavior your application needs.

## Scope and budgets

The runner starts one collector on `any` inside each of `a`, `b`, `ra`, `rb` and
`wan`, after configuring the fixture and before launching application processes.
It observes those namespaces without changing their routes or sending probes.
All collectors declare readiness before application steps start.

`--capture` defaults to 1000 packets per role. Use `--capture=5000` to set another
budget from 1 to 100000. Snapshots retain up to 256 bytes per packet, including
headers and any application data within that range. A role stops at its packet
budget; it keeps its initial packet prefix. Capture budget exhaustion does not
change the application's verdict. Repeat applies the budget separately to every
role of every case in every attempted run.

Each classic-PCAP file is bounded by `24 + packet_budget * (16 + 256)` bytes.
The five files at the default budget total at most 1,360,120 bytes per case.
Captures include ARP/IPv6 and traffic crossing multiple interfaces; one application
packet may have several records. Filters when reading the file select the protocol
and endpoints of interest. Interface indices are local to the original namespace;
an offline reader may not resolve the original interface names.

## Saved evidence and shutdown

The suite saves `capture-config.json` with version 1, kind `packet_capture_options`,
and its options. Each attempted application case saves `a.pcap`, `b.pcap`,
`ra.pcap`, `rb.pcap`, `wan.pcap`, matching `ROLE.tcpdump.log` files, and
`capture.json`. File creation uses private permissions (0600). Read them as the
caller that ran the fixture, or change ownership of the artifact directory to the
intended reader. The repository's CI workflows transfer ownership to the runner
before uploading the synthetic example evidence.

The [capture manifest schema](../schemas/packet-capture-v1.schema.json) records
packet counts, link types, snapshot truncation, packet-budget exhaustion, forced
shutdown, exit codes and the reported kernel-drop counter. `complete` describes
a cleanly closed, structurally valid capture; it does not describe the application
verdict or promise a lossless view. The kernel-drop counter is what tcpdump reports,
and may be null when unavailable. Use it alongside truncation and budget fields.

Collectors flush on normal completion, failed assertions, errors and SIGINT/SIGTERM
before fixture processes and namespaces are removed. Graceful shutdown gets two
seconds, after which remaining collectors are terminated and marked forced. An
unexpected collector exit or malformed capture fails the fixture while keeping
available files and logs. Partial startup saves a manifest for collectors that
started. SIGKILL and power loss may leave unflushed, incomplete evidence.

Completed application observations embed the capture manifest under `packet_capture`.
Failed/interrupted attempts may have no completed observation; their available
capture files and manifest remain in the case directory. Timelines include capture
readiness and completion events. `repeat` also records the options under
`environment.packet_capture`; case timings include collection startup and shutdown.
