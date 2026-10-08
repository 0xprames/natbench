#!/usr/bin/env bash
# Copy a buildable independent project; never replace an existing destination.
set -euo pipefail
if [[ $# != 1 && $# != 2 ]]; then
  echo "usage: $0 NEW_PROJECT_DIRECTORY [PACKAGE_NAME]" >&2
  exit 2
fi
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
python3 - "$repo" "$1" "${2:-custom-transport-adapter}" <<'PY'
import json,pathlib,re,shutil,sys
source=pathlib.Path(sys.argv[1]);destination=pathlib.Path(sys.argv[2]);name=sys.argv[3]
if not re.fullmatch(r'[a-z][a-z0-9_-]{0,63}',name):raise SystemExit('package name must be a safe lowercase identifier, 1–64 characters')
if destination.exists():raise SystemExit('project directory must be new')
template=source/'examples/adapter-starter'
manifest=(template/'Cargo.toml').read_text();lock=(template/'Cargo.lock').read_text()
old_name=re.search(r'^name = "([^"]+)"$',manifest,re.M).group(1)
old_version=re.search(r'^version = "([^"]+)"$',manifest,re.M).group(1)
manifest=re.sub(r'^name = "[^"]+"$',f'name = "{name}"',manifest,count=1,flags=re.M)
manifest=re.sub(r'^version = "[^"]+"$','version = "0.1.0"',manifest,count=1,flags=re.M)
manifest=manifest.replace('../../crates/transport-protocol','crates/transport-protocol')
old=f'[[package]]\nname = "{old_name}"\nversion = "{old_version}"'
if old not in lock:raise SystemExit('template lockfile does not match package')
lock=lock.replace(old,f'[[package]]\nname = "{name}"\nversion = "0.1.0"',1)
destination.mkdir()
for filename in ['build.rs']:
    shutil.copyfile(template/filename,destination/filename)
shutil.copytree(template/'src',destination/'src')
protocol=destination/'crates/transport-protocol';protocol.mkdir(parents=True)
for filename in ['Cargo.toml','Cargo.lock']:
    shutil.copyfile(source/'crates/transport-protocol'/filename,protocol/filename)
shutil.copytree(source/'crates/transport-protocol/src',protocol/'src')
shutil.copyfile(source/'LICENSE',protocol/'LICENSE');shutil.copyfile(source/'LICENSE',destination/'LICENSE')
workflow=destination/'.github/workflows';workflow.mkdir(parents=True)
shutil.copyfile(source/'examples/github-actions/adapter.yml',workflow/'adapter.yml')
(destination/'Cargo.toml').write_text(manifest);(destination/'Cargo.lock').write_text(lock)
(destination/'.gitignore').write_text('/target/\n/artifacts*/\n')
(destination/'adapter.json').write_text(json.dumps(dict(schema_version=1,kind='transport_adapter_config',adapter=dict(name='custom-transport',argv=[f'./target/release/{name}'])),indent=2)+'\n')
(destination/'README.md').write_text('''# Custom transport adapter

Build with `cargo build --release --locked`. Run `natbench conform adapter.json
--capture --artifacts ./artifacts-001` on a prepared Linux fixture host.

Replace `src/transport.rs` with your real library/socket integration. The initial
backend is plain Linux TCP with no TLS or endpoint authentication; record your
actual engine, versions, security, stream/framing and settings in `settings()`.
Keep byte/attempt/sequence verification and bounded I/O. The wrapper in
`src/main.rs` implements request validation, private atomic peer publication,
readiness, whole-client deadline, JSONL events and verified workload metrics.

The shared version-1 contract is included under `crates/transport-protocol`;
this project builds separately and has no dependency on natbench's core runner.
The source fingerprint covers this adapter's manifest, lockfile, build script and
source; retain the original reports and record additional library build metadata.

`--unsupported` demonstrates an explicit unsupported capability and exit 3.
It intentionally does not pass conformance. Conformance cannot attest to hidden
implementation behavior: an adapter must honestly verify received bytes before
claiming delivery. See https://github.com/0xprames/natbench/blob/main/docs/ADAPTER_STARTER.md.
''')
print(destination.resolve())
PY
