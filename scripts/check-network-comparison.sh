#!/usr/bin/env bash
# Verify real adapters against declared directional conditions and total loss.
set -euo pipefail
if [[ $# != 3 ]]; then
  echo "usage: $0 NATBENCH ADAPTER NEW_ARTIFACT_DIRECTORY" >&2
  exit 2
fi
umask 077
binary=$(realpath "$1")
adapter=$(realpath "$2")
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
mkdir -- "$3"
artifacts=$(realpath "$3")
python3 - "$repo" "$artifacts" "$adapter" <<'PY'
import copy,json,pathlib,sys
repo,root,adapter=pathlib.Path(sys.argv[1]),pathlib.Path(sys.argv[2]),sys.argv[3]
config=json.loads((repo/'examples/transports/conditions.json').read_text())
for item in config['adapters']:item['argv'][0]=adapter
(root/'conditions.json').write_text(json.dumps(config))
negative=copy.deepcopy(config);negative['cases']=[]
for direction in ['client_to_server','server_to_client']:
    case=copy.deepcopy(config['cases'][0]);case['name']=f'total loss {direction}';case['deadline_ms']=2000
    case['network'][direction]['loss_percent']=100
    negative['cases'].append(case)
(root/'total-loss.json').write_text(json.dumps(negative))
PY
"$binary" compare "$artifacts/conditions.json" --runs 3 --capture --artifacts "$artifacts/conditions" > "$artifacts/conditions.stdout.json"
status=0
"$binary" compare "$artifacts/total-loss.json" --runs 1 --capture --artifacts "$artifacts/total-loss" > "$artifacts/total-loss.stdout.json" || status=$?
test "$status" -eq 1
python3 - "$artifacts" <<'PY'
import json,pathlib,sys,xml.etree.ElementTree as ET
root=pathlib.Path(sys.argv[1])
positive=json.loads((root/'conditions/report.json').read_text());negative=json.loads((root/'total-loss/report.json').read_text())
assert positive['complete'] and not positive['interrupted'] and len(positive['attempts'])==36
assert all(a['outcome']=='passed' for a in positive['attempts'])
assert negative['complete'] and not negative['interrupted'] and len(negative['attempts'])==6
assert all(a['outcome']=='transport_failed' and a['measurement'] is None for a in negative['attempts'])
for label,report in [('conditions',positive),('total-loss',negative)]:
    cases={c['name']:c for c in report['case_conditions']}
    xml=ET.parse(root/label/'junit.xml').getroot()
    assert int(xml.attrib['tests'])==len(report['planned']) and int(xml.attrib['failures'])==(6 if label=='total-loss' else 0)
    for plan,attempt in zip(report['planned'],report['attempts']):
        case=root/label/plan['artifacts_directory']/'evidence/case-000'
        configured=cases[plan['case']]['network']
        evidence=json.loads((case/'network.json').read_text())
        assert evidence['configured'] and evidence['complete'] and not evidence['interrupted'] and not evidence['errors']
        assert len(evidence['links'])==2 and evidence['tc_version']
        for direction,egress,role,device in [('client_to_server','router_a_wan','ra','wan'),('server_to_client','wan_router_a','wan','a')]:
            link=next(l for l in evidence['links'] if l['egress']==egress)
            assert link['role']==role and link['device']==device and link['requested']==configured[direction]
            assert all(isinstance(link[field],list) for field in ['before','configured','after'])
            enabled=link['requested']['delay_ms']>0 or link['requested']['loss_percent']>0
            assert link['installed']==enabled
            if enabled:
                q=next(q for q in link['configured'] if q['kind']=='netem' and q['root'])
                assert q['handle']=='1:' and q['options']['limit']==4096
                observed=q['options'].get('delay',{}).get('delay',0)
                assert abs(observed-link['requested']['delay_ms']/1000)<.000001,(link,observed)
                final=next(q for q in link['after'] if q['kind']=='netem' and q['root'])
                assert final['packets']>0 or final['drops']>0
                if link['requested']['loss_percent']==100:assert final['drops']>0
            else:
                assert not link['configure_argv'] and link['queue_limit_packets'] is None
                assert not any(q['kind']=='netem' for q in link['configured'])
        assert json.loads((case/'capture.json').read_text())['complete']
        assert (case/'timeline.jsonl').exists() and (case/'client-1.stdout.log').exists()
        if label=='conditions':
            m=attempt['measurement'];assert m['bulk_verified_bytes']==65536 and len(m['message_rtt_seconds'])==5
            minimum=(configured['client_to_server']['delay_ms']+configured['server_to_client']['delay_ms'])/1000
            if minimum:
                # This checks that both directions affect application delivery;
                # it is a generous lower bound, not a transport ranking.
                assert min(m['message_rtt_seconds'])>=minimum*.8,(plan,m)
        else:
            assert attempt['messages'] and any('connect:' in message for message in attempt['messages'])
print('Verified 36 real shaped deliveries, six directional total-loss failures, kernel settings/counters and retained captures.')
PY
