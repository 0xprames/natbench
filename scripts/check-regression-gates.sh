#!/usr/bin/env bash
# Use a real comparison for the passing gates; labelled mutations exercise diagnostics.
set -euo pipefail
if [[ $# != 3 ]]; then
  echo "usage: $0 NATBENCH COMPARISON_REPORT NEW_ARTIFACT_DIRECTORY" >&2
  exit 2
fi
umask 077
binary=$(realpath "$1")
source_report=$(realpath "$2")
mkdir -- "$3"
artifacts=$(realpath "$3")
python3 - "$source_report" "$artifacts" <<'PY'
import copy,json,pathlib,sys
source=pathlib.Path(sys.argv[1]);root=pathlib.Path(sys.argv[2]);report=json.loads(source.read_text())
assert report['complete'] and not report['interrupted'] and all(a['outcome']=='passed' for a in report['attempts'])
rules=[]
for case,adapter in sorted({(p['case'],p['adapter']) for p in report['planned']}):
    rules.append(dict(case=case,adapter=adapter,min_attempts=3,min_successful_attempts=3,min_message_rtt_samples=20,min_delivery_rate=1,max_message_rtt_p95_seconds=1,min_bulk_goodput_bytes_per_second=65536))
policy=dict(schema_version=1,kind='transport_regression_policy',cohorts=rules)
(root/'absolute-policy.json').write_text(json.dumps(policy))
for rule in rules:rule['baseline']=dict(max_delivery_rate_drop=0,max_message_rtt_p95_increase_fraction=.5,max_bulk_goodput_drop_fraction=.5,allowed_metadata_changes=[])
(root/'relative-policy.json').write_text(json.dumps(policy))
mutation=copy.deepcopy(report);mutation['validation_mutation']='synthetic RTT regression for gate diagnostics; not a transport run'
for attempt in mutation['attempts']:attempt['measurement']['message_rtt_seconds']=[2]*attempt['measurement']['workload']['measured_messages']
(root/'synthetic-regression.json').write_text(json.dumps(mutation))
mutation=copy.deepcopy(report);mutation['validation_mutation']='synthetic network mismatch for compatibility diagnostics; not a transport run'
for case in mutation['case_conditions']:case['profile']='udp-blocked'
(root/'synthetic-incompatible.json').write_text(json.dumps(mutation))
PY
"$binary" assess "$source_report" --policy "$artifacts/absolute-policy.json" --artifacts "$artifacts/absolute" > "$artifacts/absolute.stdout.json"
"$binary" assess "$source_report" --policy "$artifacts/relative-policy.json" --baseline "$source_report" --artifacts "$artifacts/relative" > "$artifacts/relative.stdout.json"
status=0
"$binary" assess "$artifacts/synthetic-regression.json" --policy "$artifacts/relative-policy.json" --baseline "$source_report" --artifacts "$artifacts/regression" > "$artifacts/regression.stdout.json" || status=$?
test "$status" -eq 1
status=0
"$binary" assess "$artifacts/synthetic-incompatible.json" --policy "$artifacts/relative-policy.json" --baseline "$source_report" --artifacts "$artifacts/incompatible" > "$artifacts/incompatible.stdout.json" || status=$?
test "$status" -eq 2
python3 - "$artifacts" <<'PY'
import json,pathlib,sys,xml.etree.ElementTree as ET
root=pathlib.Path(sys.argv[1])
for name,code in [('absolute',0),('relative',0),('regression',1),('incompatible',2)]:
    report=json.loads((root/name/'report.json').read_text())
    assert report['complete'] and not report['interrupted'] and report['exit_code']==code
    assert report['current_source_exit_code']==0 and len(report['cohorts'])==6
    xml=ET.parse(root/name/'junit.xml').getroot()
    assert int(xml.attrib['tests'])==6
    assert int(xml.attrib['failures'])==(6 if code==1 else 0)
    assert int(xml.attrib['errors'])==(6 if code==2 else 0)
    assert all(c['current']['counts']['passed']==3 and c['current']['message_rtt_seconds']['samples']>=20 for c in report['cohorts'])
    assert (root/name/'current.json').is_file() and (root/name/'policy.json').is_file()
    if name=='regression':assert all(any(c['name']=='message_rtt_p95_increase_fraction' and not c['passed'] for c in cohort['checks']) for cohort in report['cohorts'])
    if name=='incompatible':assert all(any(c['field']=='case_conditions' and not c['allowed'] for c in cohort['metadata_changes']) for cohort in report['cohorts'])
print('Verified real-report absolute/self-baseline gates and labelled synthetic regression/compatibility failures, with retained raw inputs and JUnit.')
PY
