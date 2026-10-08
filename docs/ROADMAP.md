# natbench release and launch plan

natbench will become a connectivity testbed for real applications: run programs
behind controlled networks, verify their connection and recovery behavior, and
save evidence that explains failures, and compare real transport implementations
under the same workloads and network conditions. Rust is the implementation
language; tests must work with programs written in any language.

This schedule starts October 6, 2026. Dates are target completion dates in the
maintainer's America/New_York time zone. Exit criteria determine release readiness.
A missed gate moves the launch; it does not remove validation requirements.
External adoption and interoperability are dependencies, not guaranteed dates.

## Milestones and target releases

| Milestone | Target date | Release | Outcome |
| --- | --- | --- | --- |
| M1 CI foundations and distribution | October 16, 2026 | v0.1.1 | Install a binary, run declared expectations, retain CI evidence |
| M2 Application runner and transport comparison | October 30, 2026 | v0.2.0 | Compare iroh and reference/custom transports with verified application data |
| M3 Composable networks and recovery events | November 13, 2026 | v0.3.0 | Compare transport behavior under combined impairments and network changes |
| M4 Broader transport coverage and interoperability | November 27, 2026 | v0.4.0 | Add WebRTC/libp2p/TURN comparisons and compatible protocol interoperability |
| M5 Portable real network diagnosis | December 11, 2026 | v0.5.0 | Collect bounded, understandable evidence from a user's real network |

## M1 CI foundations and distribution

Exit criteria:

- Versioned scenario and report formats with explicit schema compatibility rules.
- Declarative built-in cases with assertions, useful summaries, JSON and JUnit.
- Distinct outcomes for assertion failure, fixture failure and inconclusive evidence.
- A prerequisite checker explains missing tools and namespace/NFQUEUE capabilities.
- Tagged Linux binaries, checksums and release smoke tests; source builds remain supported.
- CI tests successful expectations, expected negative results, bad input, cancellation,
  artifact preservation and resource cleanup. Failed jobs retain available artifacts.
- Document observation limits and the difference between the test relay and TURN.

Current progress: `natbench doctor` reports prerequisites and its active mode
probes a disposable kernel fixture, including optional NFQUEUE. The binary workflow
builds and smoke-tests x86-64 and ARM64 archives and checksums before tag publication.
Schema compatibility documentation and cancellation evidence tests are implemented.
Version [0.1.1 is published](https://github.com/0xprames/natbench/releases/tag/v0.1.1)
with suite report schema 2, preserved partial results, SIGINT/SIGTERM cleanup
coverage and tested x86-64/ARM64 archives. M1 is complete.

Initial implementation: `natbench test` executes a JSON suite against the existing
built-in benchmark. This is the first foundation slice, not the application runner.
The application runner, logs, packet captures and repeated reports followed in M2.

## M2 Application runner, transport comparison and first public launch

Current slices: input schema 2 adds foreground application commands, declared
stdout/TCP readiness, deadlines, sequential stop/restart, exit/stdout assertions,
per-process logs and lifecycle timelines. Schema 3 adds output/exit waits and
supervised delay steps for explicit service downtime. The independent Go UDP
example exercises NAT, restart and an expected UDP-blocked failure. A standalone
Rust example proves a persistent UDP client receives fresh application data after
a service outage behind preserve and random NATs. `natbench repeat` executes a
frozen scenario with fresh fixtures, per-verdict counts/timings, environment metadata,
and preserved evidence from interrupted attempts. Optional `--capture` adds bounded
per-role PCAPs and manifests preserved on failure or interruption. The
[adoption walkthrough](GETTING_STARTED.md), copyable Actions workflow and tested
failure/correction demo are ready for evaluation in v0.2.0-alpha.2. The
[contributor guide](../CONTRIBUTING.md) and [evaluation checklist](EVALUATION.md)
support this preview. The [transport comparison plan](TRANSPORT_COMPARISONS.md)
moves iroh, a plain QUIC reference and generic external adapters into M2. The direct-stream
[comparison runner and iroh/Quinn adapters](ADAPTERS.md) now implement normalized
application timings, verified bulk goodput, rotated runs and preserved evidence in
the v0.2.0-alpha.2 preview, whose native bundles include the example executables.
[Real local iroh connectivity cases](IROH_CONNECTIVITY.md) now
exercise two NATed peers, automatic/forced relay policy, blocked UDP and relay
interruption with verified delivery and selected-path evidence through application
scenarios. They are separate from direct-stream performance comparisons; generic
cross-stack connectivity cohorts remain further work. The earlier v0.2.0-alpha.1 predates both additions.
Outside evaluation remains a launch check. Composable network changes follow in M3; broader protocol coverage
follows in M4.

Exit criteria:

- A scenario describes peer and service commands, argv, working directories,
  environment, network roles, readiness conditions, deadlines and assertions.
- Readiness comes from a declared condition, not a guessed sleep. Every failure
  identifies the process or phase; bounded supervision collects stdout and stderr.
- Programs perform their own discovery and traversal. The fixture does not silently
  pre-punch their sockets. Optional application events expose selected-path evidence.
- Timed service stop/restart and message exchange assertions prove recovery.
- JSON/JUnit and a readable timeline accompany saved logs and optional packet capture.
- Two standalone application examples in different languages run without modifying
  natbench source. At least one outside project or maintainer evaluates the workflow.
- A documented GitHub Actions example takes a new user from install to a saved report.
- Repeated runs report counts and timing distributions, with environment metadata.
- A versioned executable-adapter/workload contract keeps transport libraries outside
  the core and accepts separately built custom implementations under a generic label.
- Iroh and a plain QUIC reference exchange the same verified payloads in matched
  direct-path tests. Actual local iroh relay/traversal cases demonstrate automatic
  versus relayed paths, blocked UDP and relay interruption with application evidence.
- Readable and versioned comparison results preserve attempts, failure phases, path
  policy, implementation versions, first-data timing, application RTT and verified
  goodput. Whole-case wall times are not labeled transport performance.
- Complete-stack comparisons expose supported capabilities separately from direct
  performance cohorts; unsupported traversal is never silently supplied by the lab.

Launch gate: another developer can reproduce a connectivity failure, identify its
phase from the artifacts, compare transport behavior with the same workload, and
use the test as a regression check. Release notes
include supported scenarios, an installation walkthrough, and explicit limitations.
Publish v0.2.0 only after this gate. External outreach requires a separately
approved message; issue/release preparation does not authorize sending messages.

## M3 Composable networks and recovery events

The [v0.3.0-alpha.1 evaluation checkpoint](releases/v0.3.0-alpha.1.md) bundles
[directional delay/random loss](NETWORK_CONDITIONS.md), a plain TCP baseline,
[offline gates](REGRESSION_GATES.md), [fresh outage recovery](TRANSPORT_RECOVERY.md)
and [independent Rust adapters](ADAPTER_STARTER.md). Native bundles and pinned CI
workflows support maintainer/outside evaluation; this does not complete M3.
The [delivery queue](DELIVERY_QUEUE.md) records the accepted network, TCP, regression
gate, recovery and adapter-starter work; the remaining M3 gates below stay open.

Exit criteria:

- Common scenario configuration replaces isolated combinations of command flags.
- Independent mapping, filtering, allocation, expiry/refresh and hairpin controls
  where supported. Kernel profiles and userspace translator policies remain explicit.
- Nested NAT, asymmetric loss/delay/jitter, MTU and rate limits compose with peers.
- Scheduled mapping expiry and address/port rebinding expose reconnection behavior.
- IPv6 and dual-stack routing have named, tested cases before being called supported.
- Record allocation/impairment seeds where supported, observed behavior, kernel and
  versions. Seeds do not imply identical wall-clock timing across hosts.
- Curated fast CI cases plus budgeted extended matrices avoid uncontrolled combinations.
- The same transport workloads exercise composed conditions; recovery results identify
  resumed versus recreated sessions and verified delivery after a network change.

## M4 Broader transport coverage and protocol interoperability

Exit criteria:

- Expand the shared-workload comparison to real WebRTC/libp2p implementations; keep
  cross-stack performance comparison separate from compatible wire interoperability.
- WebRTC gathers and selects candidates through its normal ICE machinery; tests cover
  direct connections, forced TURN fallback and ICE restart. No hidden preparatory punch.
- A separately installed coturn runs in the fixture, including authenticated allocation
  and supported UDP/TCP/TLS transport cases. Credentials stay out of published logs.
- Browser and independent server-side implementations exchange application data.
- A real libp2p example exercises DCUtR rather than the test mailbox protocol.
- QUIC scenarios exercise connection recovery or migration after NAT rebinding.
- Report selected paths with application evidence and shutdown controls; a handshake
  or signaling success alone never counts as application delivery.
- Compatibility versions are recorded and dependencies installed separately from the
  core runner where practical.

## M5 Portable diagnosis

Exit criteria:

- Unprivileged client probes work on Linux, macOS and Windows independently of the
  Linux-only network fixture. Privileged network emulation remains Linux-first.
- Explicit server configuration and bounded probes record IPv4/IPv6 reachability,
  sampled mappings, and filtering/lifetime only when server capabilities permit.
- Two-ended probes verify actual peer reachability; STUN observations alone do not.
- Reports distinguish no reply, unsupported server features and measured restrictions.
  They describe observations at a time and endpoint, not permanent NAT guarantees.
- Address-bearing reports have documented local storage and export/redaction controls.
- Diagnostic findings link to candidate lab scenarios without claiming that a synthetic
  configuration exactly reproduces an ISP or router.

## Release process and ongoing prioritization

1. Review progress weekly against the current gate and record remaining blockers in
   milestone issues. This plan does not create a recurring automation.
2. Deliver small reviewed pull requests with their scenario-level validation evidence.
3. Publish clearly labeled alpha previews for outside evaluation when the
   implementation and install/CI walkthrough pass; previews do not satisfy an
   external adoption gate. Produce a release candidate when a milestone gate is
   satisfied; exercise install, quickstart and CI on a fresh Linux environment
   before tagging a stable release.
4. Publish binaries, checksums, schema notes and a runnable example with each tag.
5. For v0.2.0, prepare a concise demo of an actual app failure and recovery plus a
   contributor guide. Broader launch follows successful outside evaluation.
6. Prioritize bugs and adoption friction discovered by users over additional standalone
   experiments. Measure repeat use, external integrations and diagnosed regressions.

Background: [Tailscale connectivity testing](https://github.com/tailscale/tailscale/issues/13038),
[Pion vnet](https://github.com/pion/transport/blob/master/vnet/README.md),
[libp2p DCUtR](https://github.com/libp2p/specs/blob/master/relay/DCUtR.md),
[coturn](https://github.com/coturn/coturn), and
[RFC 5780 observation limits](https://www.rfc-editor.org/rfc/rfc5780.html).
