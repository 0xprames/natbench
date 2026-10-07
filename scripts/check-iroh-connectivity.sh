#!/usr/bin/env bash
# Run real two-peer iroh connectivity through ordinary application scenarios.
set -euo pipefail
if [[ $# != 3 ]]; then
  echo "usage: $0 NATBENCH IROH_CONNECTIVITY NEW_ARTIFACT_DIRECTORY" >&2
  exit 2
fi
umask 077
binary=$(realpath "$1")
peer=$(realpath "$2")
mkdir -- "$3"
artifacts=$(realpath "$3")
python3 - "$artifacts" "$peer" <<'PY'
import json, pathlib, secrets, sys
root, peer = pathlib.Path(sys.argv[1]), sys.argv[2]
specs = [
    ('automatic-preserve', 'preserve', 'preserve', 'automatic', 'direct', False, False),
    ('automatic-random', 'random', 'random', 'automatic', 'any', False, False),
    ('automatic-blocked', 'udp-blocked', 'udp-blocked', 'automatic', 'relay', False, False),
    ('automatic-one-blocked', 'preserve', 'udp-blocked', 'automatic', 'relay', False, False),
    ('forced-relay', 'preserve', 'preserve', 'relay_only', 'relay', False, False),
    ('direct-survives-relay-stop', 'preserve', 'preserve', 'automatic', 'direct', True, False),
    ('forced-relay-stop', 'preserve', 'preserve', 'relay_only', 'relay', True, True),
    ('blocked-relay-stop', 'udp-blocked', 'udp-blocked', 'automatic', 'relay', True, True),
]
(root / 'matrix.json').write_text(json.dumps(specs, indent=2))
for name, a, b, policy, expected, interrupt, failure in specs:
    directory = root / name
    directory.mkdir()
    base = dict(schema_version=1, kind='iroh_connectivity_request', run_id=secrets.token_hex(16),
                policy=policy, expected_path=expected, interrupt_relay=interrupt,
                peer_file=str(directory / 'peer.json'), certificate_file=str(directory / 'relay.der'),
                resume_file=str(directory / 'resume.json'), deadline_ms=12000,
                workload=dict(payload_bytes=128, warmup_messages=2, measured_messages=10, bulk_bytes=262144))
    for role in ['server', 'client']:
        (directory / f'{role}.json').write_text(json.dumps(dict(base, role=role), indent=2))
    relay = dict(name='relay', role='wan', argv=[peer,'relay','--certificate',base['certificate_file']],
                 timeout_seconds=15, ready=dict(kind='stdout_contains',text='"event":"ready"'))
    server = dict(name='server', role='b', argv=[peer,'peer','--request',str(directory / 'server.json')],
                  timeout_seconds=15, ready=dict(kind='stdout_contains',text='"event":"ready"'))
    client = dict(name='client', role='a', argv=[peer,'peer','--request',str(directory / 'client.json')], timeout_seconds=15)
    processes = [relay, server, client]
    steps = [dict(action='start',process='relay'),dict(action='start',process='server')]
    if interrupt:
        client['ready'] = dict(kind='stdout_contains',text='"event":"before_relay_stop"')
        processes.append(dict(name='resume',role='wan',argv=[peer,'resume','--request',str(directory / 'client.json')],timeout_seconds=2))
        steps += [dict(action='start',process='client'),dict(action='stop',process='relay'),
                  dict(action='run',process='resume'),dict(action='wait_exit',process='client',timeout_seconds=5,
                  expect_exit=1 if failure else 0,stdout_contains='"event":"failed"' if failure else '"event":"completed"')]
    else:
        steps += [dict(action='run',process='client',stdout_contains='"event":"completed"'),dict(action='stop',process='relay')]
    steps.append(dict(action='stop',process='server'))
    scenario = dict(schema_version=3,cases=[dict(name=name,a=a,b=b,router_input='drop',processes=processes,steps=steps)])
    (directory / 'scenario.json').write_text(json.dumps(scenario,indent=2))
PY
status=0
while IFS= read -r name; do
  directory="$artifacts/$name"
  result=0
  "$binary" test "$directory/scenario.json" --capture --artifacts "$directory/evidence" > "$directory/stdout.json" || result=$?
  if [[ $result -ne 0 ]]; then status=1; fi
done < <(python3 - "$artifacts/matrix.json" <<'PY'
import json,sys
for spec in json.load(open(sys.argv[1])): print(spec[0])
PY
)
python3 - "$artifacts" <<'PY'
import json, pathlib, sys, xml.etree.ElementTree as ET
root = pathlib.Path(sys.argv[1])
summary = dict(schema_version=1,kind='iroh_connectivity_suite',complete=False,attempts=[])
def selected(evidence):
    paths = [p['kind'] for p in evidence['paths'] if p['selected']]
    assert len(paths) == 1, evidence
    return paths[0]
def events(path):
    return [json.loads(line) for line in path.read_text().splitlines()]
for name, a, b, policy, expected, interrupt, failure in json.loads((root/'matrix.json').read_text()):
    directory, case = root/name, root/name/'evidence/case-000'
    request = json.loads((directory/'client.json').read_text())
    report = json.loads((directory/'evidence/report.json').read_text())
    assert report['complete'] and not report['interrupted'] and report['cases'][0]['status']=='passed', report
    assert ET.parse(directory/'evidence/junit.xml').getroot().attrib['failures']=='0'
    client, server = events(case/'client-1.stdout.log'), events(case/'server-1.stdout.log')
    for e in client+server:
        assert e['schema_version']==1 and e['kind']=='iroh_connectivity_event' and e['run_id']==request['run_id'], e
        assert e['iroh_version']=='1.3.0' and e['relay_version']=='1.3.0' and e['policy']==policy, e
        assert len(e['adapter_source_sha256'])==64
    assert len({e['adapter_source_sha256'] for e in client+server})==1
    assert json.loads((case/'capture.json').read_text())['complete']
    assert len(list(case.glob('*.pcap')))==5 and (case/'timeline.jsonl').exists()
    bootstrap = json.loads((directory/'peer.json').read_text())
    assert set(bootstrap)=={'schema_version','kind','run_id','policy','endpoint_id','relay_url'}
    assert bootstrap['run_id']==request['run_id'] and bootstrap['relay_url']=='https://198.18.0.1:8443/'
    assert len([e for e in client if e['event']=='connected'])==1
    for e in client:
        evidence = e['detail'].get('path_evidence')
        if e['event'] in ('connected','path_observed','after_relay_stop'): evidence=e['detail']
        if evidence is not None:
            assert evidence['source']=='iroh Connection::paths' and evidence['remote_id']==bootstrap['endpoint_id']
    verified = [e for e in client if e['event']=='verified']
    received = [e for e in server if e['event']=='received']
    n = 2+request['workload']['warmup_messages']+request['workload']['measured_messages']
    if interrupt:
        before = next(e for e in client if e['event']=='before_relay_stop')
        assert selected(before['detail']['path_evidence'])==expected
        after_index = next(i for i,e in enumerate(client) if e['event']=='after_relay_stop')
        assert json.loads((directory/'resume.json').read_text())==dict(run_id=request['run_id'],relay_stopped=True)
        timeline = events(case/'timeline.jsonl')
        stopped = next(i for i,e in enumerate(timeline) if e.get('process')=='relay' and e.get('state')=='stopped')
        resumed = next(i for i,e in enumerate(timeline) if e.get('process')=='resume' and e.get('state')=='started')
        assert stopped < resumed
        if not failure:
            assert [e['detail']['sequence'] for e in client[after_index+1:] if e['event']=='verified']==list(range(1,n))
            assert all(selected(e['detail']['path_evidence'])=='direct' for e in client[after_index+1:] if e['event']=='verified')
    if failure:
        assert client[-1]['event']=='failed' and client[-1]['detail']['phase']=='exchange_after_relay_stop', client
        assert [e['detail']['sequence'] for e in verified]==[0]
        assert [e['detail']['sequence'] for e in received]==[0]
        path = expected
        outcome = 'expected_relay_outage'
    else:
        assert client[-1]['event']=='completed', client
        assert [e['detail']['sequence'] for e in verified]==list(range(n))
        assert [e['detail']['sequence'] for e in received]==list(range(n))
        final = client[-1]['detail']
        assert final['same_connection'] and final['verified_exchanges']==n and final['bulk_verified_bytes']==request['workload']['bulk_bytes']
        assert received[-1]['detail']['verified_bytes']==request['workload']['bulk_bytes']
        path = selected(final['path_evidence'])
        assert final['selected_path']==path and (expected=='any' or path==expected)
        outcome = 'verified_delivery'
    if policy=='relay_only':
        assert all(all(p['kind']=='relay' for p in e['detail']['path_evidence']['paths']) for e in verified)
    summary['attempts'].append(dict(case=name,run_id=request['run_id'],a=a,b=b,policy=policy,
        outcome=outcome,selected_path=path,relay_interrupted=interrupt,
        verified_exchanges=len(verified),bulk_verified_bytes=0 if failure else request['workload']['bulk_bytes'],
        adapter_source_sha256=client[-1]['adapter_source_sha256'],evidence_directory=f'{name}/evidence'))
    (root/'summary.json').write_text(json.dumps(summary,indent=2))
    print(f'{name}: {outcome}, selected {path}, {len(verified)} verified exchanges')
(root/'summary.json').write_text(json.dumps(summary,indent=2))
PY
test "$status" -eq 0
python3 - "$artifacts" "$binary" <<'PY'
import json, pathlib, secrets, subprocess, sys, time
root, binary = pathlib.Path(sys.argv[1]), sys.argv[2]
source, directory = root/'direct-survives-relay-stop', root/'cancelled'
directory.mkdir()
run_id = secrets.token_hex(16)
for role in ['server','client']:
    config = json.loads((source/f'{role}.json').read_text().replace(str(source),str(directory)))
    config['run_id']=run_id
    (directory/f'{role}.json').write_text(json.dumps(config,indent=2))
scenario = json.loads((source/'scenario.json').read_text().replace(str(source),str(directory)))
scenario['cases'][0]['name']='cancel live iroh connection'
# Hold the controller gate so cancellation cannot race a completed workload.
scenario['cases'][0]['steps']=scenario['cases'][0]['steps'][:3]+[dict(action='wait_stdout',process='client',text='controller never resumes this case',timeout_seconds=15)]
(directory/'scenario.json').write_text(json.dumps(scenario,indent=2))
def namespaces():
    result=subprocess.run(['ip','netns','list'],check=True,capture_output=True,text=True)
    return {line.split()[0] for line in result.stdout.splitlines() if line.startswith('nb')}
before=namespaces()
with (directory/'stdout.json').open('w') as out, (directory/'stderr.log').open('w') as err:
    process=subprocess.Popen([binary,'test',str(directory/'scenario.json'),'--capture','--artifacts',str(directory/'evidence')],stdout=out,stderr=err)
    try:
        log=directory/'evidence/case-000/client-1.stdout.log'
        deadline=time.monotonic()+20
        while not (log.exists() and '"event":"before_relay_stop"' in log.read_text()):
            assert process.poll() is None, 'fixture exited before cancellation gate'
            assert time.monotonic()<deadline, 'timed out waiting for live connection'
            time.sleep(.025)
        process.terminate()
        assert process.wait(timeout=10)==130
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
report=json.loads((directory/'evidence/report.json').read_text())
assert report['interrupted'] and not report['complete'], report
capture=json.loads((directory/'evidence/case-000/capture.json').read_text())
assert capture['complete'] and len(list((directory/'evidence/case-000').glob('*.pcap')))==5
assert namespaces()==before, 'namespace leak after cancellation'
for path in pathlib.Path('/proc').glob('[0-9]*/cmdline'):
    try: args=path.read_bytes()
    except (FileNotFoundError,PermissionError,ProcessLookupError): continue
    assert not (b'natbench-iroh-connectivity' in args and str(directory).encode() in args), 'peer/relay process leak'
summary=json.loads((root/'summary.json').read_text())
summary['cancellation']=dict(exit_code=130,interrupted=True,capture_complete=True,namespaces_cleaned=True,evidence_directory='cancelled/evidence')
summary['complete']=True
(root/'summary.json').write_text(json.dumps(summary,indent=2))
print('Cancelled a live iroh connection: exit 130, retained captures, no owned namespace or process leaks.')
PY
