# natbench

Measure NAT behavior and exercise UDP traversal, TCP fallback, and recovery
in isolated Linux networks. Bring your own program with a small Rust fixture.

NAT labels alone don't tell you whether peers can connect. `natbench` observes
public mappings, probes inbound filtering separately, attempts simultaneous
UDP hole punching, and removes discovery and relay processes to check whether
a direct path actually works without them.

This is an experimental correctness harness, not a throughput benchmark or a
complete RFC conformance suite. It ships as a Rust binary using Linux network
namespaces and nftables. The same executable supplies the UDP endpoints and relay.
No application daemon or container runtime is required.

## Quick start

Requirements: Linux, root, iproute2, nftables, and conntrack. Build with a current stable
Rust toolchain; the resulting binary has no Python or Rust runtime dependency.
On Debian/Ubuntu, from the repository root:

```sh
sudo apt-get install iproute2 nftables conntrack
cargo build --release --locked
sudo ./target/release/natbench bench
sudo ./target/release/natbench bench --a preserve --b random
sudo ./target/release/natbench matrix > results.json
sudo ./target/release/natbench bench --router-input accept
sudo ./target/release/natbench lifetime
sudo ./target/release/natbench collision
sudo ./target/release/natbench hairpin
sudo ./target/release/natbench impair
sudo ./target/release/natbench impair --loss-percent 100
sudo ./target/release/natbench nested
sudo ./target/release/natbench translate
sudo ./target/release/natbench stun
sudo ./target/release/natbench quic
sudo ./target/release/natbench webrtc
sudo ./target/release/natbench run --role a -- ip route
```

Alternatively, `cargo install --path . --locked` installs the command. Use the
absolute path to the installed executable with sudo if it is not on root's PATH.
This project has not been published to crates.io.

`bench` emits one JSON result; `matrix` emits all nine ordered profile pairs.
A failed traversal is a measured result and exits successfully. Setup or endpoint
failures exit nonzero. Use the Rust API or inspect JSON for your own CI policy;
there is no universal expected outcome for arbitrary NATs.

## Network fixture

```text
client a 10.1.0.2 -- router a 10.1.0.1 | 198.18.0.10 --+
                                                    |-- public bridge
client b 10.2.0.2 -- router b 10.2.0.1 | 198.18.0.20 --+
                                                       observers: 198.18.0.1/.2
                                                       TCP relay: 198.18.0.1:9100
```

Every run creates unique namespaces. Clients have no direct route to each other's
private subnet. Forwarding, firewall rules, and sysctls are set inside the fixture;
the parent network's routes and firewall are not changed. Veth endpoints briefly
exist in the parent namespace while being moved. Programs share the host filesystem
and are not sandboxed by this network fixture.

| Profile | Configuration |
| --- | --- |
| `preserve` | Kernel masquerade, preserving the source port when available |
| `random` | Kernel masquerade with `fully-random` allocation |
| `udp-blocked` | Kernel masquerade with forwarded UDP dropped; TCP remains allowed |

Profiles are independently chosen for each router. They describe configuration,
not a promise of a NAT taxonomy. JSON includes the kernel version and observed
mapping/filtering classes. Observations cover one client socket and three remote
endpoints, not every possible destination or allocation collision.

Router WAN input defaults to dropping unsolicited traffic. `--router-input accept`
lets you explore how router-local conntrack state affects hole punching.

## What an experiment checks

1. Both clients exchange payloads through a minimal TCP mailbox relay.
2. A single UDP source socket contacts the same remote IP on two ports, then a
   second IP. Public source endpoints reveal mapping reuse.
3. A fresh source socket contacts only one observer. Probes from another IP,
   another port on the same IP, and the original endpoint measure filtering.
4. Clients repeatedly send to each other's observer-reported endpoints from the
   same UDP sockets used for discovery. The harness records reception on each side.
5. The TCP relay and UDP observers stop. Relay requests must fail; if traversal
   succeeded, fresh UDP payloads must still arrive in both directions.
6. The relay restarts and another exchange verifies recovery.

The controller passes discovered endpoints over local process pipes. There is no
STUN implementation or discovery protocol under test. The relay is an unauthenticated
in-memory test mailbox; it demonstrates TCP reachability and restart recovery, not
reliable delivery, security, or a production relay implementation. A restart uses
new payloads, not durable retry of messages queued before an outage.

If forwarded UDP is blocked, mapping/filtering are `unobserved`; absence of packets
is not enough to infer a NAT class. `--timeout` sets the hole-punch attempt window
(default 2 seconds, maximum 3600); control exchanges can add a small amount of time
to that window.

## Mapping lifetime

`lifetime` is a separate experiment, so `matrix` stays a traversal run. It sets
both UDP conntrack timers on router A to the same value (`--udp-timeout`, default
3 seconds, allowed range 2–60). Linux uses one timer before a reply is seen and
another after; equal values make a single idle gap apply to either timer.

On one mapping, the run then checks three patterns. The table is read with
`conntrack -L` in the router namespace when that program is installed, and from
`/proc/net/nf_conntrack` otherwise. Some kernels omit that proc file inside a
namespace. A port-preserving NAT can recreate the same public endpoint, so the
table is what shows whether the entry survived.

1. No further packets. After the timeout, the entry should be gone, and an inbound
   probe to the old public endpoint should not arrive.
2. Outbound keepalives on the original socket, for longer than the timeout. The
   entry should remain, and a probe should arrive.
3. One outbound packet to create the mapping, then only inbound keepalives from
   the observer. The entry should remain, and a further probe should arrive.

`udp-blocked` reports `established: false` and null for the three phases.
This records Linux conntrack's behavior. It does not select a refresh policy.

## Port collision

`collision` puts a second client behind router A and binds both to source port
10000. Both send to the same observer. A port-preserving NAT gives the second
flow a different public port and leaves the first entry in place. A
port-overloading NAT would hand that public port to the second flow and drop
the first. The result says which one happened. The public port after a later
packet from the first client is included, because a reissued port is not proof
that the original entry survived.

## Hairpin

`hairpin` puts a second client on router A's LAN, at 10.1.0.3. Each client
learns its public mapping from the observer, then sends to the other client's
public address from the same socket. The result says whether that packet is
dropped (`no-hairpin`), arrives with the sender's private address
(`internal-source`), or arrives with the sender's public address
(`external-source`). Linux masquerade on this fixture does not rewrite that
packet back onto the LAN.

## Loss and delay

`impair` runs one preserve/preserve traversal. Mapping discovery and the TCP
relay baseline happen first. Then `tc netem` is attached to both router WAN
interfaces, and the hole punch, the direct-path check, and relay recovery run
under that impairment. Each direction is delayed once, on the sending router's
egress, so the round trip is twice `--delay-ms` (default 20, maximum 1000).
`--loss-percent` defaults to 0 and cannot exceed 100. A total loss still
reports the NAT class observed before the drop.

## Nested NAT

`nested` runs the usual preserve/preserve measurement with a second NAT in
front of client A. Router A's WAN is the private address 10.8.0.2. A new
router masquerades that onto 198.18.0.10, the address client A used to publish
directly. Client B is unchanged. The result is the normal bench JSON with
`nested_a` set, so the public mapping and the hole punch are observations of
the two NATs composed, not of the inner router alone.

## Userspace translator

`translate` moves UDP off kernel masquerade. Each router queues UDP to NFQUEUE
42 and runs `nat --lan-interface lan --wan-interface wan`. TCP masquerade stays
in nftables, so the relay baseline still uses the kernel. With no
`--translator` path, the command is this binary. Its mapping is
endpoint-independent, it keeps the source port when that port is free, and it
allows return traffic from any address once the mapping exists. Linux
masquerade does not: its filtering stays address-and-port-dependent. The result
is the normal bench JSON with `translator` set to the program that ran.

`--translator` can be another program with that same command, including a
separately installed [natlab](https://github.com/danderson/natlab) binary.
natbench does not vendor that code. The probes record whatever the program
does.

## STUN

`stun` runs the usual preserve/preserve measurement, but each client learns its
public mapping with a STUN Binding request on the same socket it later uses to
punch. The lab server, on the observer addresses, answers with
XOR-MAPPED-ADDRESS. There is no authentication. The recorded endpoint is the
address the client decoded, and the hole punch uses that address. Filtering
probes stay plaintext, so the mapping class and the filter class are still
separate observations.

## QUIC

`quic` starts a QUIC server on client B and a QUIC client on client A in the
preserve/preserve fixture. Each binds UDP port 10000, learns its public address
with a STUN Binding request on that socket, and publishes the mapped address
through the TCP relay. Both send a datagram at the peer's mapped address so the
address-and-port-dependent filter will admit the handshake. They then complete
a QUIC handshake on that same socket and exchange application data. The
controller stops the relay, and they exchange a second payload on the same
connection. The result records both exchanges. This is a connectivity check,
not a throughput measurement. The server certificate is generated for the lab
and is not a trust anchor.

## WebRTC

`webrtc` opens a data channel between the same preserve/preserve peers. Each
binds UDP port 10000, learns its public address with a STUN Binding request on
that socket, and publishes the mapped address through the TCP relay. Both send
a datagram at the peer's mapped address so the address-and-port-dependent
filter will admit the handshake. They exchange ICE credentials and the DTLS
fingerprint over the relay, then complete ICE, DTLS, and SCTP on that socket.
The data channel carries one payload. The controller stops the relay, and they
exchange a second payload on the same channel. The result records both
exchanges. This is a connectivity check, not a media or throughput test. The
DTLS certificate is generated for the lab and is not a trust anchor.

## Bring your own programs

```rust,no_run
use natbench::lab::{Lab, Profile, RouterInput};

fn main() -> anyhow::Result<()> {
    let mut lab = Lab::create(Profile::Preserve, Profile::Random, RouterInput::Drop)?;
    let _server = lab.spawn("b", &["./your-server", "--listen", "0.0.0.0:7000"])?;
    // Arrange discovery/readiness for your protocol, then run the client:
    let routes = lab.run("a", &["ip", "route"])?;
    println!("{}", String::from_utf8_lossy(&routes.stdout));
    println!("{:?}", lab.namespaces); // Roles: a, b, ra, rb, wan
    Ok(())
} // Lab's Drop terminates tracked children and removes namespaces.
```

`spawn` inherits stdio; `run` captures output and checks exit status. Keep the `Lab`
alive while using its processes. Its `Drop` implementation cleans up after normal
returns, errors, and unwinding panics. The CLI also handles SIGINT/SIGTERM and exits
130 after cleanup. Embedded applications must install their own signal handling;
they can register `natbench::cancellation()` with signal-hook.

SIGKILL, aborting panics, or machine failure can leave resources behind: inspect
`ip netns list` for namespaces beginning with `nb`, confirm ownership, then remove
them manually. Cleanup also kills child processes still in this run's namespaces.
This fixture owns the lifecycle of programs launched into it.

## Lessons that shaped the design

- **Mapping and filtering are different.** In our initial Linux run, the `preserve`
  profile reused a public endpoint across destinations but allowed inbound packets
  only from the contacted address and port. Calling it a “full cone NAT” would be wrong.
- **Router input policy matters.** A peer's early inbound probe can create router-local
  conntrack state. That can force the client's outbound probe onto a different source
  port, invalidating the endpoint discovered earlier. The input-policy experiment
  reproduces this failure even when source-port preservation is configured.
- **Use one socket for discovery and traversal.** A different socket gets a different
  NAT mapping. The example deliberately retains the original UDP source socket.
- **Remove infrastructure to prove independence.** A successful send while a relay is
  running does not prove a direct path. Shutdown and recovery are explicit checks.
- **Test ordered, asymmetric pairs.** A symmetric-only profile list misses combinations
  where only one side changes allocation or blocks UDP.
- **Report observations rather than assume outcomes.** Kernel versions, collisions,
  timing, and topology can change behavior. Results contain measured endpoints.

## Relationship to danderson/natlab

[danderson/natlab](https://github.com/danderson/natlab) explores NAT emulation with
userspace UDP translation through NFQUEUE and port allocation policies. It is useful
related work. The kernel profiles remain the default measurement. `translate` adds
a userspace UDP path on the same probes.

`natbench` adds an automated measurement and connectivity experiment layer:
asymmetric matrices, independent mapping/filtering observations, infrastructure
shutdown controls, TCP fallback/recovery, machine-readable results, and a reusable
program fixture. Kernel profiles exercise Linux conntrack and nftables. The
userspace translator is a separate UDP path and is not a copy of natlab.

This is a separate implementation, not a fork. It contains no danderson/natlab code.
`translate --translator` can launch a separately installed natlab binary inside
each router namespace and subject it to the same observations. Documented NAT
policies are verified by probes rather than assumed. That program remains under
its own GPL-3.0 license.

## Scope and next experiments

Implemented: IPv4 UDP observations and traversal, IPv4 TCP relay controls, three
kernel profiles, asymmetric matrices, generic process execution, JSON results,
lifecycle tests, mapping expiry and refresh observations, port-collision
observations, hairpin observations, WAN loss and delay observations, a nested
NAT in front of client A, a userspace UDP translator with endpoint-independent
filtering, STUN Binding discovery on the traversal socket, a QUIC handshake
on that socket, and a WebRTC data channel on that socket. Both application
checks still carry data after the relay is gone.

IPv6, TCP hole punching, and performance measurement are outside the current suite.

## Development

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
# Compile as your normal account, then run privileged test binaries via Cargo:
sudo env "PATH=$PATH" "CARGO_HOME=$HOME/.cargo" "RUSTUP_HOME=$HOME/.rustup" \
  cargo test --locked -- --include-ignored --test-threads=1
```

Integration tests exercise all nine profile pairs, shutdown/recovery, the router
input collision, mapping expiry and refresh, port collision, hairpinning, WAN loss and delay, nested NAT, userspace translation, STUN discovery, QUIC handshake and relay shutdown, WebRTC data channel and relay shutdown, partial setup failure, process cleanup, and SIGINT/SIGTERM handling. Namespace tests are marked
ignored by default; the privileged invocation above explicitly enables them.
Tests require namespace and network administration privileges even when running
as root in a container. GitHub Actions runs on Ubuntu and uploads the JSON matrix.

MIT licensed. Personal project maintained by [0xprames](https://github.com/0xprames).
