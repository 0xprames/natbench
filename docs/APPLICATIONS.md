# Run applications in a NAT fixture

Application scenarios use input schema version 2. They run ordinary executables
in the same five-role Linux fixture as the built-in experiments. Programs perform
their own discovery, handshake and data transfer. The runner supplies network
configuration, process lifecycle, readiness checks, deadlines and artifacts.

This is the first M2 slice. It supports sequential start/run/stop/restart steps,
stdout or TCP readiness, exit-code and stdout assertions, per-process logs and a
JSONL timeline. Timed network events, stdin/application event adapters, packet
capture, repeated-run statistics and outside adoption remain milestone work.

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

See [scenario-v2.schema.json](../schemas/scenario-v2.schema.json) for fields and
[the example](../examples/udp-echo/scenario.json) for a complete runnable file.
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

`start` launches a foreground service with declared readiness. It must stay alive
until stopped; unexpected service exits fail the case. `run` executes a bounded
command and expects exit 0 by default; `expect_exit` overrides that value with an exit code from 0 to 255. Optional
`stdout_contains` asserts an exact byte substring. `stop` terminates a started
service's process group. `restart` stops a started service and launches a new
instance, checking readiness again. Remaining services are stopped at case end.

These steps perform hard stops, including child processes in the managed process
group. Programs must remain foreground services rather than daemonizing or escaping
supervision. Fixture teardown also removes processes still in its namespaces.

## Readiness and deadlines

A service needs one of:

- `{"kind":"stdout_contains","text":"READY"}`: the exact bytes must appear in
  captured stdout while the service remains alive. Output must be flushed.
- `{"kind":"tcp","address":"127.0.0.1:8080"}`: open and close a TCP connection
  from the service's own network namespace. The address must be literal IPv4 with
  a nonzero port. The probe is real traffic; choose stdout readiness if a connection
  would interfere with the service protocol.

No readiness sleep is guessed. A timeout or missing executable is a fixture failure;
an unexpected command exit or absent asserted stdout is an assertion failure.
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

The fixture isolates networking. Programs execute as the caller (currently root),
share the host filesystem, and inherit environment except declared overrides.
Run trusted programs/scenarios; keep credentials out of scenario files and logs
before exporting artifacts. A process can produce large logs; the runner bounds
memory used for matching but does not impose a disk quota in this slice.
