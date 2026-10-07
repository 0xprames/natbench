# Contributing to natbench

natbench is a personal MIT-licensed project. Start with a small reproducible
connectivity requirement: a program, network configuration, expected application
behavior, and observed result. Issues about adoption friction and diagnosed
regressions help prioritize work against the [roadmap](docs/ROADMAP.md).

For application integrations, use [versioned scenarios](docs/APPLICATIONS.md)
before changing the Rust runner. Keep discovery, traversal and delivery in the
application; readiness and fresh-data assertions should explain what passed.
The [preview walkthrough](docs/GETTING_STARTED.md) and
[evaluation checklist](docs/EVALUATION.md) provide a starting point.

## Develop and verify

Install a current stable Rust toolchain, Go (CI uses 1.24), and Linux tools:

```sh
sudo apt-get install -y iproute2 nftables conntrack tcpdump
go build -o examples/udp-echo/udp-echo examples/udp-echo/main.go
rustc --edition 2021 -D warnings examples/udp-recovery/main.rs -o examples/udp-recovery/udp-recovery
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
sudo env "PATH=$PATH" "CARGO_HOME=$HOME/.cargo" "RUSTUP_HOME=$HOME/.rustup" \
  cargo test --locked -- --include-ignored --test-threads=1
```

Privileged tests require namespace and network administration capabilities, even
in a root container. Run them serially on an isolated Linux host. The fixture
owns its children and namespaces; programs share the filesystem and caller's
privileges. Build as your normal account before using sudo for fixture tests.

For pull requests, describe the requirement and changed behavior, include relevant
validation, and keep unrelated work separate. Add meaningful coverage for changed
behavior and failure/cleanup paths. Input formats reject unknown fields; output
readers accept additive fields and check schema/completion status. Update schemas
and compatibility documentation together when a format changes.

Evidence should report observations and their limits. A synthetic NAT profile,
short timing sample or handshake does not establish production connectivity.
Do not contribute proprietary code or internal artifacts; use independently
reproducible examples suitable for a public MIT-licensed repository.
