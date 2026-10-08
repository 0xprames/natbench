#!/usr/bin/env bash
# Prove real same-connection recovery and preserve negative attempts.
set -euo pipefail
if [[ $# != 3 ]]; then
  echo "usage: $0 NATBENCH RECOVERY_ADAPTER NEW_ARTIFACT_DIRECTORY" >&2
  exit 2
fi
umask 077
binary=$(realpath "$1")
adapter=$(realpath "$2")
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
mkdir -- "$3"
artifacts=$(realpath "$3")
python3 - "$repo" "$artifacts" "$adapter" <<'PY'
import json,pathlib,sys
repo,root,adapter=pathlib.Path(sys.argv[1]),pathlib.Path(sys.argv[2]),sys.argv[3]
config=json.loads((repo/'examples/transports/recovery.json').read_text())
for case in config['cases']:
    for process in case['processes']:process['argv'][0]=adapter
(root/'recovery.json').write_text(json.dumps(config))
PY
"$binary" test "$artifacts/recovery.json" --capture --artifacts "$artifacts/runs" > "$artifacts/report.stdout.json"
python3 - "$artifacts" <<'PY'
import json,pathlib,sys,xml.etree.ElementTree as ET
root=pathlib.Path(sys.argv[1]);read=lambda p:json.loads(p.read_text())
report=read(root/'runs/report.json');config=read(root/'recovery.json')
assert report['complete'] and not report['interrupted'] and len(report['cases'])==12
assert all(case['status']=='passed' for case in report['cases'])
assert int(ET.parse(root/'runs/junit.xml').getroot().attrib['failures'])==0
run_ids=set();measurements=[]
for index,(case,planned) in enumerate(zip(report['cases'],config['cases'])):
    folder=root/'runs'/f'case-{index:03}'
    network=read(folder/'network.json');assert network['schema_version']==2 and network['complete'] and not network['errors']
    transitions=network['transitions'];permanent='permanent' in case['name'];assert len(transitions)==(1 if permanent else 2)
    events=[json.loads(line) for line in (folder/'client-1.stdout.log').read_text().splitlines()]
    assert events[0]['event']=='initial_delivered' and events[1]['event']=='probe_started'
    run_id=events[0]['run_id'];assert run_id not in run_ids;run_ids.add(run_id)
    for transition in transitions:
        assert transition['run_id']==run_id and transition['complete'] and transition['error'] is None
        assert transition==read(folder/(transition['id']+'.json'))
        assert transition['started_monotonic_ns']<=transition['applied_monotonic_ns']
        assert all(link['applied'] and isinstance(link['before'],list) and isinstance(link['after'],list) for link in transition['links'])
    last=events[-1];assert all(e['run_id']==run_id for e in events)
    if permanent:
        assert last['event']=='failed' and last['details']['phase']=='outage_probe'
        assert not any(e['event']=='recovered' for e in events)
        dropped=network['links']
        assert sum(q.get('drops',0) for link in dropped for q in link['after'])>0
    else:
        assert last['event']=='recovered';m=last['details'];restore=transitions[1]
        assert m['restored_applied_monotonic_ns']==restore['applied_monotonic_ns']
        assert m['outage_applied_monotonic_ns']==transitions[0]['applied_monotonic_ns']
        assert m['fresh_delivered_monotonic_ns']>=m['fresh_sent_monotonic_ns']>=restore['applied_monotonic_ns']
        delta=(m['fresh_delivered_monotonic_ns']-restore['applied_monotonic_ns'])/1e9
        assert 0<delta<=10 and abs(m['event_to_fresh_delivery_seconds']-delta)<1e-9
        assert m['existing_connection_survived'] and m['connection_attempts']==1 and m['path']=='direct'
        assert m['fresh_sequence']==2 and m['bulk_sequence']==3 and m['payload_verified_bytes']==128 and m['bulk_verified_bytes']==16384
        assert m['path_before'] and m['path_after']
        assert sum(q.get('drops',0) for link in restore['links'] for q in link['before'])>0
        assert not any(q['kind']=='netem' for link in restore['links'] for q in link['after'])
        measurements.append({'case':case['name'],'run_id':run_id,**m})
    assert read(folder/'capture.json')['complete']
    timeline=[json.loads(line) for line in (folder/'timeline.jsonl').read_text().splitlines()]
    assert sum(e['state']=='network_change_applied' for e in timeline)==len(transitions)
(root/'verified-recoveries.json').write_text(json.dumps({'schema_version':1,'kind':'verified_reference_recoveries','recoveries':measurements,'expected_persistent_outage_failures':3},indent=2)+'\n')
print('Verified nine real same-connection recoveries, three expected persistent-outage failures, fresh bytes, kernel drops and captures.')
PY
