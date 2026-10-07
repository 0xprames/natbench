#!/usr/bin/env bash
# Exercise the public adapters as ordinary external application programs.
set -euo pipefail
if [[ $# != 3 ]]; then
  echo "usage: $0 NATBENCH ADAPTER NEW_ARTIFACT_DIRECTORY" >&2
  exit 2
fi
binary=$(realpath "$1")
adapter=$(realpath "$2")
mkdir -- "$3"
artifacts=$(realpath "$3")
python3 - "$artifacts" "$adapter" <<'PY'
import json, pathlib, secrets, sys
root, adapter = pathlib.Path(sys.argv[1]), sys.argv[2]
workload = dict(payload_bytes=128, warmup_messages=2, measured_messages=5, bulk_bytes=262144)
for transport in ['iroh', 'quinn']:
    for profile in ['preserve', 'random', 'udp-blocked']:
        directory = root / f'{transport}-{profile}'
        directory.mkdir()
        run_id = secrets.token_hex(16)
        base = dict(schema_version=1, kind='transport_request', run_id=run_id,
                    peer_address='198.18.0.1:9443', peer_file=str(directory / 'peer.json'),
                    deadline_ms=3000, workload=workload)
        for role in ['server', 'client']:
            request = dict(base, role=role, listen_address='0.0.0.0:9443' if role == 'server' else '0.0.0.0:0')
            (directory / f'{role}.json').write_text(json.dumps(request))
        def process(role):
            result = dict(name=role, role='wan' if role == 'server' else 'a',
                          argv=[adapter, '--transport', transport, '--request', str(directory / f'{role}.json')],
                          timeout_seconds=6)
            if role == 'server': result['ready'] = dict(kind='stdout_contains', text='"event":"ready"')
            return result
        case = dict(name=f'{transport} direct stream on {profile}', a=profile, b='preserve', router_input='drop',
                    processes=[process('server'),process('client')],
                    steps=[dict(action='start',process='server'),
                           dict(action='run',process='client',expect_exit=1 if profile=='udp-blocked' else 0),
                           dict(action='stop',process='server')])
        (directory / 'scenario.json').write_text(json.dumps(dict(schema_version=2,cases=[case])))
PY
for transport in iroh quinn; do
  for profile in preserve random udp-blocked; do
    directory="$artifacts/$transport-$profile"
    "$binary" test "$directory/scenario.json" --capture --artifacts "$directory/evidence" > "$directory/stdout.json"
  done
done
python3 - "$artifacts" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
for directory in sorted(root.iterdir()):
    case = directory / 'evidence/case-000'
    request = json.loads((directory / 'client.json').read_text())
    events = [json.loads(line) for line in (case / 'client-1.stdout.log').read_text().splitlines()]
    assert len(events) == 1
    event = events[0]
    assert event['schema_version'] == 1 and event['kind'] == 'transport_event'
    assert event['run_id'] == request['run_id']
    assert json.loads((case / 'capture.json').read_text())['complete']
    if directory.name.endswith('udp-blocked'):
        assert event['event'] == 'failed' and event['phase'] == 'connect', event
    else:
        assert event['event'] == 'completed', event
        m = event['measurement']
        assert m['workload'] == request['workload'] and m['path'] == 'direct'
        assert len(m['message_rtt_seconds']) == 5 and m['bulk_verified_bytes'] == 262144
        assert 0 < m['first_data_seconds'] <= 3 and 0 < m['bulk_seconds'] <= 3
        assert all(0 < sample <= 3 for sample in m['message_rtt_seconds'])
    print(directory.name, event['event'])
PY
