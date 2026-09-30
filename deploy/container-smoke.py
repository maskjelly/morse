#!/usr/bin/env python3
"""Verify the actual Compose limits, nonroot volumes, command adapter and restart."""
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

root = Path(__file__).resolve().parent.parent
image = sys.argv[1] if len(sys.argv) > 1 else 'morse:local'
subprocess.run(['docker', 'image', 'inspect', image], check=True, stdout=subprocess.DEVNULL)
project = 'morse-smoke-' + secrets.token_hex(4)
with tempfile.TemporaryDirectory(prefix=project) as directory:
    temp = Path(directory)
    token = secrets.token_hex(32)
    envfile = temp / '.env'
    envfile.write_text('MORSE_TOKEN=' + token + '\n')
    envfile.chmod(0o600)
    subprocess.run(['bash', str(root / 'deploy/init-env.sh')], check=True, cwd=root)
    config = json.loads(subprocess.check_output([
        'docker', 'compose', '-p', project, '--env-file', str(envfile), '-f', str(root / 'compose.yaml'),
        'config', '--format', 'json'], cwd=root, text=True))
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    service = config['services']['morse']
    service.pop('build', None)
    service['image'] = image
    service['ports'] = [{'host_ip': '127.0.0.1', 'target': 7800, 'published': str(port), 'protocol': 'tcp'}]
    service['environment'].update(MORSE_PROVIDER='command', MORSE_AGENT_PROGRAM='/bin/echo', MORSE_AGENT_ARGS='["{prompt}"]')
    composefile = temp / 'compose.json'
    composefile.write_text(json.dumps(config))
    composefile.chmod(0o600)
    command = ['docker', 'compose', '-p', project, '-f', str(composefile)]
    base = f'http://127.0.0.1:{port}'

    def request(path, body=None, authenticated=True):
        headers = {'Authorization': 'Bearer ' + token} if authenticated else {}
        data = json.dumps(body).encode() if body is not None else None
        with urllib.request.urlopen(urllib.request.Request(base + path, data=data, headers=headers), timeout=3) as response:
            raw = response.read()
            return json.loads(raw) if path != '/healthz' else raw

    def ready():
        for _ in range(200):
            try:
                request('/healthz')
                return
            except OSError:
                time.sleep(.1)
        raise RuntimeError('container not ready')

    def run(sid, prompt):
        request(f'/api/sessions/{sid}/instruction', {'text': prompt})
        for _ in range(200):
            events = request(f'/api/sessions/{sid}/events')['events']
            if any(e['type'] == 'output' and e['chunk'] == prompt + '\n' for e in events) and request(f'/api/sessions/{sid}')['status'] == 'idle':
                assert any(e['type'] == 'tool_result' and e['ok'] for e in events)
                return events[-1]['seq']
            time.sleep(.05)
        raise RuntimeError('agent did not complete')

    try:
        subprocess.run(command + ['up', '-d', '--no-build', '--pull', 'never'], check=True, stdout=subprocess.DEVNULL)
        ready()
        container = subprocess.check_output(command + ['ps', '-q', 'morse'], text=True).strip()
        assert subprocess.check_output(['docker', 'exec', container, 'id', '-u'], text=True).strip() == '10001'
        try:
            request('/api/sessions', authenticated=False)
            raise AssertionError('unauthenticated request accepted')
        except urllib.error.HTTPError as error:
            assert error.code == 401
        sid = request('/api/sessions', {'workspace': '/home/morse/projects/smoke'})['id']
        seq = run(sid, 'fresh volume: quotes \' " $(touch injected)')
        subprocess.run(['docker', 'exec', container, 'test', '!', '-e', '/home/morse/projects/smoke/injected'], check=True)
        subprocess.run(command + ['restart', 'morse'], check=True, stdout=subprocess.DEVNULL)
        ready()
        assert sid in [s['id'] for s in request('/api/sessions')]
        assert request(f'/api/sessions/{sid}/events')['last_seq'] >= seq
        assert run(sid, 'after restart') > seq
        print('PASS: real Compose, nonroot fresh volumes, literal external agent, auth and persisted restart')
    finally:
        subprocess.run(command + ['down', '--volumes', '--remove-orphans'], stdout=subprocess.DEVNULL, check=False)
