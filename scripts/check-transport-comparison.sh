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
"$binary" compare "$artifacts/direct.json" --runs 3 --capture --artifacts "$artifacts/direct" > "$artifacts/direct.stdout.json"
status=0
"$binary" compare "$artifacts/blocked.json" --runs 1 --capture --artifacts "$artifacts/blocked" > "$artifacts/blocked.stdout.json" || status=$?
test "$status" -eq 1
python3 - "$artifacts" <<'PY'
import json,pathlib,sys,xml.etree.ElementTree as ET
root=pathlib.Path(sys.argv[1])
direct=json.loads((root/'direct/report.json').read_text())
blocked=json.loads((root/'blocked/report.json').read_text())
assert direct['complete'] and not direct['interrupted'] and len(direct['attempts'])==18
assert all(a['outcome']=='passed' for a in direct['attempts'])
for case in {p['case'] for p in direct['planned']}:
    plans=[p for p in direct['planned'] if p['case']==case]
    for position in range(3):
        assert {p['adapter'] for p in plans[position::3]}=={'iroh','quinn','tcp'}
assert blocked['complete'] and len(blocked['attempts'])==3
assert all(a['outcome']==('passed' if p['adapter']=='tcp' else 'transport_failed') for p,a in zip(blocked['planned'],blocked['attempts']))
assert all(a['measurement'] is None for p,a in zip(blocked['planned'],blocked['attempts']) if p['adapter']!='tcp')
assert all(s['first_data_seconds'] is None for s in blocked['summaries'] if s['adapter']!='tcp')
assert next(a for p,a in zip(blocked['planned'],blocked['attempts']) if p['adapter']=='tcp')['measurement']['bulk_verified_bytes']==1048576
for label,report in [('direct',direct),('blocked',blocked)]:
    xml=ET.parse(root/label/'junit.xml').getroot()
    assert int(xml.attrib['tests'])==len(report['planned'])
    assert int(xml.attrib['failures'])==(2 if label=='blocked' else 0)
    for plan,attempt in zip(report['planned'],report['attempts']):
        case=root/label/plan['artifacts_directory']/'evidence/case-000'
        assert json.loads((case/'capture.json').read_text())['complete']
        assert (case/'client-1.stdout.log').exists() and (case/'timeline.jsonl').exists()
        if plan['adapter']=='tcp':
            impl=attempt['implementation'];settings=impl['settings']
            assert impl['name']=='tcp' and impl['version']==report['environment']['kernel_release']
            assert settings['authentication']=='none' and settings['encryption']=='none; plain TCP'
            assert settings['tcp_nodelay']=='true; both endpoints' and settings['tcp_congestion_control'] and '\0' not in settings['tcp_congestion_control']
            assert 'quic_engine' not in settings and 'crypto_provider' not in settings
            sockets=attempt['measurement']['path_evidence']
            for key in ['client','server_listener']:
                socket=sockets[key]
                assert socket['tcp_nodelay'] and socket['tcp_congestion_control']==settings['tcp_congestion_control']
                assert socket['send_buffer_bytes']>0 and socket['receive_buffer_bytes']>0
            assert sockets['client']['local'] and sockets['client']['remote']
            server=[json.loads(line) for line in (case/'server-1.stderr.log').read_text().splitlines()]
            assert [s['phase'] for s in server]==['accepted','verified']
            assert all(s['socket']['tcp_nodelay'] and s['socket']['remote'] for s in server)
print('Verified matched iroh/Quinn/TCP workloads, rotated order, delivery metrics, blocked outcomes and complete diagnostic evidence.')
PY
