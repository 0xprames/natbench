#!/usr/bin/env bash
# Bundle already-built native binaries; never replace existing release evidence.
set -euo pipefail
if [[ $# != 3 && $# != 4 ]]; then
  echo "usage: $0 BINARY TARGET OUTPUT_DIRECTORY [ADAPTER_BINARY_DIRECTORY]" >&2
  exit 2
fi
binary=$(realpath "$1")
target=$2
case "$target" in
  x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu) ;;
  *) echo "unsupported release target: $target" >&2; exit 2 ;;
esac
case "$target:$(uname -m)" in
  x86_64-unknown-linux-gnu:x86_64|aarch64-unknown-linux-gnu:aarch64) ;;
  *) echo "packaging requires the target's native host" >&2; exit 2 ;;
esac
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
version=$("$binary" --version)
version=${version#natbench }
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?$ ]]; then
  echo "unexpected binary version: $version" >&2
  exit 2
fi
adapters=$(realpath "${4:-$repo/examples/transports/target/release}")
for name in natbench-transport-adapters natbench-iroh-connectivity; do
  test -x "$adapters/$name"
  if [[ "$("$adapters/$name" --version)" != "$name $version" ]]; then
    echo "adapter version differs from natbench: $name" >&2
    exit 2
  fi
done
mkdir -p -- "$3"
output=$(realpath "$3")
name="natbench-${version}-${target}"
archive="$output/$name.tar.gz"
if [[ -e "$archive" || -e "$archive.sha256" ]]; then
  echo "release archive or checksum already exists: $archive" >&2
  exit 2
fi
stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT
mkdir -p -- "$stage/$name"
install -m 755 -- "$binary" "$stage/$name/natbench"
mkdir -p -- "$stage/$name/bin"
install -m 755 -- "$adapters/natbench-transport-adapters" "$adapters/natbench-iroh-connectivity" "$stage/$name/bin/"
cp -- "$repo/LICENSE" "$repo/README.md" "$repo/CHANGELOG.md" "$repo/CONTRIBUTING.md" "$stage/$name/"
cp -R -- "$repo/docs" "$repo/scenarios" "$repo/schemas" "$stage/$name/"
mkdir -p "$stage/$name/examples/udp-echo"
cp "$repo/examples/udp-echo/main.go" "$repo/examples/udp-echo/scenario.json" "$repo/examples/udp-echo/failure.json" "$repo/examples/udp-echo/fixed.json" "$stage/$name/examples/udp-echo/"
mkdir -p "$stage/$name/examples/udp-recovery"
cp "$repo/examples/udp-recovery/main.rs" "$repo/examples/udp-recovery/scenario.json" "$stage/$name/examples/udp-recovery/"
mkdir -p "$stage/$name/examples/github-actions" "$stage/$name/scripts"
cp "$repo/examples/github-actions/application.yml" "$repo/examples/github-actions/transports.yml" "$stage/$name/examples/github-actions/"
cp "$repo/scripts/check-application-demo.sh" "$stage/$name/scripts/"
mkdir -p "$stage/$name/examples/transports/src" "$stage/$name/crates/transport-protocol/src"
cp "$repo/examples/transports/Cargo.toml" "$repo/examples/transports/Cargo.lock" "$repo/examples/transports/build.rs" "$repo/examples/transports/direct.json" "$repo/examples/transports/conditions.json" "$stage/$name/examples/transports/"
cp "$repo/examples/transports/src/main.rs" "$repo/examples/transports/src/connectivity.rs" "$stage/$name/examples/transports/src/"
cp "$repo/crates/transport-protocol/Cargo.toml" "$repo/crates/transport-protocol/Cargo.lock" "$stage/$name/crates/transport-protocol/"
cp "$repo/crates/transport-protocol/src/lib.rs" "$stage/$name/crates/transport-protocol/src/"
cp "$repo/scripts/check-transport-adapters.sh" "$repo/scripts/check-transport-comparison.sh" "$repo/scripts/check-iroh-connectivity.sh" "$repo/scripts/check-network-comparison.sh" "$stage/$name/scripts/"
# Resolve the shipped comparison against the bundled executables, retaining the
# source example's workload and adapter arguments.
python3 - "$stage/$name/examples/transports" <<'PYCONFIG'
import json,pathlib,sys
for name in ['direct.json','conditions.json']:
    path=pathlib.Path(sys.argv[1])/name;config=json.loads(path.read_text())
    for adapter in config['adapters']: adapter['argv'][0]='../../bin/natbench-transport-adapters'
    path.write_text(json.dumps(config,indent=2)+'\n')
PYCONFIG
tar -czf "$archive" -C "$stage" "$name"
(cd -- "$output" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
printf '%s\n' "$archive"
