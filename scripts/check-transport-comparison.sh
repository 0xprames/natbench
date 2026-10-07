#!/usr/bin/env bash
set -euo pipefail
if [[ $# != 3 ]]; then
  echo "usage: $0 NATBENCH ADAPTER NEW_ARTIFACT_DIRECTORY" >&2
  exit 2
fi
binary=$(realpath "$1")
adapter=$(realpath "$2")
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
mkdir -- "$3"
artifacts=$(realpath "$3")
python3 - "$repo" "$artifacts" "$adapter" <<'PY'
import json,pathlib,sys
repo,root,adapter = pathlib.Path(sys.argv[1]),pathlib.Path(sys.argv[2]),sys.argv[3]
config = json.loads((repo/'examples/transports/direct.json').read_text())
for item in config['adapters']: item['argv'][0]=adapter
# Exercise stream credit reuse and application progress beyond the initial stream window.
for case in config['cases']:
    case['workload']['measured_messages']=120
    case['deadline_ms']=2500
(root/'direct.json').write_text(json.dumps(config))
config['cases']=config['cases'][:1]
config['cases'][0]['name']='blocked direct transport comparison'
config['cases'][0]['profile']='udp-blocked'
config['cases'][0]['deadline_ms']=2000
(root/'blocked.json').write_text(json.dumps(config))
PY
"$binary" compare "$artifacts/direct.json" --runs 2 --capture --artifacts "$artifacts/direct" > "$artifacts/direct.stdout.json"
status=0
"$binary" compare "$artifacts/blocked.json" --runs 1 --capture --artifacts "$artifacts/blocked" > "$artifacts/blocked.stdout.json" || status=$?
test "$status" -eq 1
python3 - "$artifacts" <<'PY'
import json,pathlib,sys,xml.etree.ElementTree as ET
root=pathlib.Path(sys.argv[1])
direct=json.loads((root/'direct/report.json').read_text())
blocked=json.loads((root/'blocked/report.json').read_text())
assert direct['complete'] and not direct['interrupted'] and len(direct['attempts'])==8
assert all(a['outcome']=='passed' for a in direct['attempts'])
assert blocked['complete'] and len(blocked['attempts'])==2
assert all(a['outcome']=='transport_failed' and a['measurement'] is None for a in blocked['attempts'])
assert all(s['first_data_seconds'] is None for s in blocked['summaries'])
for label,report in [('direct',direct),('blocked',blocked)]:
    xml=ET.parse(root/label/'junit.xml').getroot()
    assert int(xml.attrib['tests'])==len(report['planned'])
    assert int(xml.attrib['failures'])==(2 if label=='blocked' else 0)
    for plan in report['planned']:
        case=root/label/plan['artifacts_directory']/'evidence/case-000'
        assert json.loads((case/'capture.json').read_text())['complete']
        assert (case/'client-1.stdout.log').exists() and (case/'timeline.jsonl').exists()
print('Verified matched iroh/Quinn workloads, rotated order, delivery metrics, blocked outcomes and complete diagnostic evidence.')
PY
