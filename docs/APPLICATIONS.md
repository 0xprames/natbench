# Run applications in a NAT fixture

Application scenarios use input schema versions 2 and 3. They run ordinary executables
in the same five-role Linux fixture as the built-in experiments. Programs perform
their own discovery, handshake and data transfer. The runner supplies network
configuration, process lifecycle, readiness checks, deadlines and artifacts.

The v0.2.0-alpha.1 preview supports sequential start/run/stop/restart steps,
stdout or TCP readiness, exit-code and stdout assertions, per-process logs and a
JSONL timeline. Schema 3 adds bounded output/exit waits and explicit downtime.
Optional [`--capture`](PACKETS.md) saves bounded packet evidence from every fixture role.
Repeated runs retain counts/timings and per-attempt evidence. See the
[adoption walkthrough](GETTING_STARTED.md) and [evaluation checklist](EVALUATION.md).
Outside evaluation remains the M2 stable launch gate; composable network events
and richer protocol integrations follow in M3/M4. Structured events/stdin are not
part of this preview.

## Independent Go UDP example

```sh
go build -o examples/udp-echo/udp-echo examples/udp-echo/main.go
cargo build --locked
sudo ./target/debug/natbench test examples/udp-echo/scenario.json --artifacts ./udp-run-001
```

The executable uses Go's standard UDP sockets and has no natbench dependency. Its
server runs on the WAN; two clients run behind preserve and random NATs. The
scenario verifies a request, restarts the server, then verifies another request.
A second case blocks forwarded UDP and expects the application's nonzero exit.
It checks service availability after restart; persistent-client recovery and peer
discovery need their own application tests. Go is only needed to build this example.

## Scenario model

See [scenario-v2.schema.json](../schemas/scenario-v2.schema.json) for the original
application fields and [schema 3](../schemas/scenario-v3.schema.json) for waits
and delays. The [Go example](../examples/udp-echo/scenario.json) and
[Rust example](../examples/udp-recovery/scenario.json) are complete runnable files.
Unknown fields are rejected. Version 1 built-in suites continue to work.

Each case names router profiles and declares up to 32 processes and 100 steps.
The fixture has no Internet uplink; put local signaling, rendezvous or relay
services in `wan` rather than expecting access to production cloud services.
Process names are unique ASCII letters, digits, underscores or hyphens, limited
to 64 characters. Roles are `a`, `b`, `ra`, `rb`, and `wan`. The clients' private
addresses are 10.1.0.2 and 10.2.0.2; WAN services may use 198.18.0.1.

A process provides `argv` and `timeout_seconds` (greater than zero, at most 3600).
Optional `cwd` defaults to the scenario's directory; relative paths resolve there,
including process executable paths. Absolute paths are allowed. Optional `env`
overrides inherited environment variables. A custom PATH applies to the application
without changing how the runner finds fixture tools. No shell is added implicitly;
use an explicit shell command when that is part of the intended program.

`start` launches a foreground process with declared readiness. Services stay alive
until stopped; unexpected service exits fail the case. In schema 3, a launch whose
next terminal step is `wait_exit` is an asynchronous command: it may finish after
readiness, and its result is retained until that step. A terminal step is `stop`,
`restart` or `wait_exit`; each launch is handled independently. `run` executes a bounded
command and expects exit 0 by default; `expect_exit` overrides that value with an exit code from 0 to 255. Optional
`stdout_contains` asserts an exact byte substring. `stop` terminates a started
service's process group. `restart` stops a started service and launches a new
instance, checking readiness again. Remaining services are stopped at case end.

These steps perform hard stops, including child processes in the managed process
group. Programs must remain foreground services rather than daemonizing or escaping
supervision. Fixture teardown also removes processes still in its namespaces.

## Persistent-client recovery in Rust

```sh
rustc --edition 2021 examples/udp-recovery/main.rs -o examples/udp-recovery/udp-recovery
cargo build --locked
sudo ./target/debug/natbench test examples/udp-recovery/scenario.json --artifacts ./recovery-001
```

The standalone Rust program uses standard UDP sockets and has no natbench library
dependency. The client opens one socket and keeps it for the entire attempt. It
declares `CONNECTED` after receiving matching application data. The scenario stops
the WAN echo service, waits for the client's `DISCONNECTED` output, leaves the
service down for an additional 0.2 seconds, then starts it again. The client must
receive three consecutive fresh replies before declaring recovery and exiting.
Both preserve and random NAT profiles are exercised. The logs retain the client's
local endpoint through the attempt. This demonstrates application recovery across
a service outage; NAT rebinding and persistent-peer traversal are later scenarios.

Schema 3 adds these steps:

| Action | Fields | Behavior |
| --- | --- | --- |
| `wait_stdout` | `process`, `text`, `timeout_seconds` | Wait for an exact byte substring in the current launch's stdout |
| `wait_exit` | `process`, `timeout_seconds`, optional `expect_exit`/`stdout_contains` | Collect a started command, check its exit/output, and remove it from the active set |
| `delay` | `seconds` | Hold the current configuration for an explicit duration while supervising services |

Wait deadlines begin when the step starts. They and delays must be finite, greater
than zero and at most 3600 seconds. `wait_stdout` searches the entire current
launch's log, including prior output; repeated waits for the same marker can match
the same occurrence. Use distinct application markers for successive phases.
When the producing command has exited, available output is still matched.
An absent marker before exit or deadline is an assertion failure (exit 1).
An exit deadline is a fixture failure (exit 2). Wrong exit/output assertions fail
with exit 1, matching `run` behavior. `wait_exit` accepts already-completed commands.

Readiness, waits and delays supervise other services and check cancellation.
Delay and wait events appear in the saved timeline. Times record runner observations;
they do not claim exact kernel exit times or precise scheduling. A delay specifies
an intended outage duration; readiness still comes from the declared condition.

## Readiness and deadlines

A service needs one of:

- `{"kind":"stdout_contains","text":"READY"}`: the exact bytes must appear in
  captured stdout while the service remains alive. Output must be flushed.
- `{"kind":"tcp","address":"127.0.0.1:8080"}`: open and close a TCP connection
  from the service's own network namespace. The address must be literal IPv4 with
  a nonzero port. The probe is real traffic; choose stdout readiness if a connection
  would interfere with the service protocol.

No readiness sleep is guessed. Readiness/execution timeouts or missing executables
are fixture failures; an unexpected command exit or absent asserted stdout is an assertion failure.
Errors identify the failing step and process. Log matching scans bounded chunks
and checks deadlines/cancellation instead of loading unlimited output into memory.

## Artifacts and execution context

Each case uses `case-000`, `case-001`, etc. Process launches produce
`NAME-GENERATION.stdout.log` and `.stderr.log`; restart uses a new generation.
`timeline.jsonl` records relative times, process names and lifecycle states.
Completed observations include events and the case log directory. Failed or
interrupted attempts keep available logs and timelines even when no completed
observation exists. Suite JSON/JUnit checkpoint and exit semantics remain at report
version 2; see [schema compatibility](SCHEMAS.md).

Use [`natbench repeat`](REPETITIONS.md) to run the same captured application
scenario several times with fresh fixtures and retained evidence from every attempt.
The aggregate report includes verdict counts and case timing distributions by verdict.

The fixture isolates networking. Programs execute as the caller (currently root),
share the host filesystem, and inherit environment except declared overrides.
Run trusted programs/scenarios; keep credentials out of scenario files and logs
before exporting artifacts. A process can produce large logs; the runner bounds
memory used for matching but does not impose a disk quota in this slice.
