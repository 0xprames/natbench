#!/usr/bin/env bash
# Bundle an already-built native binary; never replace existing release evidence.
set -euo pipefail
if [[ $# != 3 ]]; then
  echo "usage: $0 BINARY TARGET OUTPUT_DIRECTORY" >&2
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
cp -- "$repo/LICENSE" "$repo/README.md" "$repo/CHANGELOG.md" "$repo/CONTRIBUTING.md" "$stage/$name/"
cp -R -- "$repo/docs" "$repo/scenarios" "$repo/schemas" "$stage/$name/"
mkdir -p "$stage/$name/examples/udp-echo"
cp "$repo/examples/udp-echo/main.go" "$repo/examples/udp-echo/scenario.json" "$repo/examples/udp-echo/failure.json" "$repo/examples/udp-echo/fixed.json" "$stage/$name/examples/udp-echo/"
mkdir -p "$stage/$name/examples/udp-recovery"
cp "$repo/examples/udp-recovery/main.rs" "$repo/examples/udp-recovery/scenario.json" "$stage/$name/examples/udp-recovery/"
mkdir -p "$stage/$name/examples/github-actions" "$stage/$name/scripts"
cp "$repo/examples/github-actions/application.yml" "$stage/$name/examples/github-actions/"
cp "$repo/scripts/check-application-demo.sh" "$stage/$name/scripts/"
mkdir -p "$stage/$name/examples/transports/src" "$stage/$name/crates/transport-protocol/src"
cp "$repo/examples/transports/Cargo.toml" "$repo/examples/transports/Cargo.lock" "$repo/examples/transports/build.rs" "$repo/examples/transports/direct.json" "$stage/$name/examples/transports/"
cp "$repo/examples/transports/src/main.rs" "$stage/$name/examples/transports/src/"
cp "$repo/crates/transport-protocol/Cargo.toml" "$repo/crates/transport-protocol/Cargo.lock" "$stage/$name/crates/transport-protocol/"
cp "$repo/crates/transport-protocol/src/lib.rs" "$stage/$name/crates/transport-protocol/src/"
cp "$repo/scripts/check-transport-adapters.sh" "$repo/scripts/check-transport-comparison.sh" "$stage/$name/scripts/"
tar -czf "$archive" -C "$stage" "$name"
(cd -- "$output" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
printf '%s\n' "$archive"
