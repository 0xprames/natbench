#!/usr/bin/env bash
# Verify a diagnosed application failure and its passing regression scenario.
set -euo pipefail
if [[ $# != 2 ]]; then
  echo "usage: $0 NATBENCH NEW_ARTIFACT_DIRECTORY" >&2
  exit 2
fi
binary=$(realpath "$1")
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
mkdir -- "$2"
artifacts=$(realpath "$2")
"$binary" doctor --probe --json > "$artifacts/doctor.json"
status=0
"$binary" test "$repo/examples/udp-echo/failure.json" --capture \
  --artifacts "$artifacts/failure" > "$artifacts/failure.stdout.json" || status=$?
if [[ $status != 1 ]]; then
  echo "expected an application assertion failure (exit 1), got $status" >&2
  exit 1
fi
"$binary" repeat "$repo/examples/udp-echo/fixed.json" --runs 2 --capture \
  --artifacts "$artifacts/fixed" > "$artifacts/fixed.stdout.json"
python3 - "$artifacts" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
failed = json.loads((root / 'failure/report.json').read_text())
assert failed['complete'] and not failed['interrupted']
assert failed['cases'][0]['status'] == 'assertion_failed'
fixed = json.loads((root / 'fixed/summary.json').read_text())
assert fixed['complete'] and fixed['completed_runs'] == 2 and not fixed['interrupted']
for run in sorted((root / 'fixed').glob('run-*')):
    report = json.loads((run / 'report.json').read_text())
    assert report['cases'][0]['status'] == 'passed'
manifests = list(root.glob('failure/case-*/capture.json')) + list(root.glob('fixed/run-*/case-*/capture.json'))
assert len(manifests) == 3
for path in manifests:
    capture = json.loads(path.read_text())
    assert capture['complete'] and len(capture['files']) == 5
PY
for pair in failure/case-000/a failure/case-000/wan fixed/run-001/case-000/wan; do
  name=${pair//\//-}
  tcpdump -nn -r "$artifacts/$pair.pcap" 'udp and host 198.18.0.1 and port 9999' \
    > "$artifacts/$name.udp.txt" 2> "$artifacts/$name.decode.log"
done
test -s "$artifacts/failure-case-000-a.udp.txt"
test ! -s "$artifacts/failure-case-000-wan.udp.txt"
grep -q '198.18.0.1.9999 >' "$artifacts/fixed-run-001-case-000-wan.udp.txt"
echo "Verified: blocked application failure, local outgoing UDP, no WAN UDP, and two passing corrected runs."
