#!/usr/bin/env bash
# Real socket conformance for the bundled starter; unsupported stays nonpassing.
set -euo pipefail
if [[ $# != 3 ]]; then
  echo "usage: $0 NATBENCH ADAPTER NEW_ARTIFACT_DIRECTORY" >&2
  exit 2
fi
umask 077
binary=$(realpath "$1")
adapter=$(realpath "$2")
mkdir -- "$3"
artifacts=$(realpath "$3")
python3 - "$artifacts" "$adapter" <<'PY'
import copy,json,pathlib,sys
root=pathlib.Path(sys.argv[1]);adapter=sys.argv[2]
config=dict(schema_version=1,kind='transport_adapter_config',adapter=dict(name='custom-transport',argv=[adapter]))
(root/'adapter.json').write_text(json.dumps(config))
unsupported=copy.deepcopy(config);unsupported['adapter']['argv'].append('--unsupported')
(root/'unsupported.json').write_text(json.dumps(unsupported))
PY
"$binary" conform "$artifacts/adapter.json" --capture --artifacts "$artifacts/conformance" > "$artifacts/conformance.stdout.json"
status=0
"$binary" conform "$artifacts/unsupported.json" --capture --artifacts "$artifacts/unsupported" > "$artifacts/unsupported.stdout.json" || status=$?
test "$status" -eq 3
python3 - "$artifacts" <<'PY'
import json,pathlib,sys,xml.etree.ElementTree as ET
root=pathlib.Path(sys.argv[1])
for name,code in [('conformance',0),('unsupported',3)]:
    report=json.loads((root/name/'report.json').read_text());raw=json.loads((root/name/'runs/report.json').read_text())
    plan=json.loads((root/name/'runs/plan.json').read_text())
    assert report['kind']=='transport_adapter_conformance_report' and report['complete'] and not report['interrupted'] and report['exit_code']==code
    assert raw['kind']=='transport_adapter_conformance_runs' and raw['complete'] and len(raw['attempts'])==8
    assert plan['kind']=='transport_adapter_conformance_plan' and len(plan['adapters'])==1
    assert len(report['verdicts'])==8 and all(v['passed']==(code==0) for v in report['verdicts'])
    if code==0:
        assert sum(a['outcome']=='passed' for a in raw['attempts'])==6
        assert sum(a['outcome']=='transport_failed' and a['measurement'] is None for a in raw['attempts'])==2
        maximum=[a['measurement'] for p,a in zip(raw['planned'],raw['attempts']) if p['case'].startswith('maximum')]
        assert len(maximum)==2 and all(m['bulk_verified_bytes']==16777216 and len(m['message_rtt_seconds'])==1000 for m in maximum)
    else:
        assert all(a['outcome']=='unsupported' and a['measurement'] is None for a in raw['attempts'])
    xml=ET.parse(root/name/'junit.xml').getroot()
    assert int(xml.attrib['tests'])==8 and int(xml.attrib['failures'])==0 and int(xml.attrib['errors'])==0
    assert int(xml.attrib['skipped'])==(8 if code==3 else 0)
    for p in raw['planned']:
        case=root/name/'runs'/p['artifacts_directory']/'evidence/case-000'
        assert json.loads((case/'capture.json').read_text())['complete']
        assert json.loads((case/'network.json').read_text())['complete']
print('Verified six real starter deliveries including all workload bounds, two expected total-loss failures, and eight nonpassing unsupported cases with retained evidence.')
PY
