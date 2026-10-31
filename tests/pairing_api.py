import contextlib
"""One-use pairing, persistence, expiry, revocation and operator boundaries."""
import hashlib
import json
import os
import pathlib
import socket
import sqlite3
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

with tempfile.TemporaryDirectory(prefix='xenia-pairing-') as tmp:
    db = pathlib.Path(tmp) / 'db'
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    root = 'synthetic-controller-pairing-token'
    env = os.environ | {'XENIA_DB':str(db), 'XENIA_TOKEN':root, 'XENIA_LISTEN':f'127.0.0.1:{port}'}
    binary = os.environ.get('XENIA_TEST_BINARY', 'target/debug/xenia')
    def request(route, body=None, token=root, expected=200):
        req = urllib.request.Request(f'http://127.0.0.1:{port}/api/'+route, None if body is None else json.dumps(body).encode(), {'Authorization':'Bearer '+token, 'Content-Type':'application/json'})
        try:
            with urllib.request.urlopen(req, timeout=3) as r:
                assert r.status == expected
                if route.startswith('pairing/'):
                    assert r.headers['Cache-Control'] == 'no-store'
                return json.load(r)
        except urllib.error.HTTPError as e:
            assert e.code == expected, (route, e.code, expected)
    def start():
        proc = subprocess.Popen([binary, 'controller'], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(100):
            try:
                request('state', token=env['XENIA_TOKEN'])
                return proc
            except OSError:
                time.sleep(.05)
        proc.terminate(); proc.wait(timeout=5)
        raise AssertionError('controller startup failed')
    def create():
        result = request('pairing/create', {'origin':'https://controller.example:8443'})
        assert '<svg' in result['svg'] and root not in result['url']
        assert 110 < result['expires'] - time.time() <= 120
        return result['url'].split('#pair=')[1]
    def sql(query, args=()):
        with contextlib.closing(sqlite3.connect(db)) as connection, connection:
            return connection.execute(query, args).fetchall()
    p = start()
    try:
        request('pairing/create', {'origin':'https://controller.example'}, token='', expected=401)
        worker = request('workers/enroll', {'id':'worker'})['token']
        request('pairing/create', {'origin':'https://controller.example'}, token=worker, expected=401)
        for origin in ['http://controller.example','https://controller.example/path','https://user@controller.example','https://controller.example/?x=1','https://controller.example/#x']:
            request('pairing/create', {'origin':origin}, expected=400)
        old = create(); code = create()
        request('pairing/redeem', {'code':old}, token='', expected=400)
        credential = request('pairing/redeem', {'code':code}, token='')['token']
        assert credential != root and credential != code
        assert sql('select digest from paired_devices')[0][0] == hashlib.sha256(credential.encode()).hexdigest()
        request('pairing/redeem', {'code':code}, token='', expected=400)
        request('state', token=credential)
        request('heartbeat', {'id':'worker'}, token=credential, expected=401)
        p.terminate(); p.wait(timeout=5); p = start()
        request('state', token=credential)
        device = request('pairing/devices', {})[0]
        request('pairing/revoke', {'id':device['id']}, token=worker, expected=401)
        request('pairing/revoke', {'id':device['id']})
        request('state', token=credential, expected=401)
        code = create(); sql('update pairing_codes set expires=0')
        request('pairing/redeem', {'code':code}, token='', expected=400)
        code = create(); request('pairing/cancel', {})
        request('pairing/redeem', {'code':code}, token='', expected=400)
        code = create(); credential = request('pairing/redeem', {'code':code}, token='')['token']
        request('pairing/end', {}, token=credential)
        request('state', token=credential, expected=401)
        code = create(); credential = request('pairing/redeem', {'code':code}, token='')['token']
        sql('update paired_devices set expires=0')
        request('state', token=credential, expected=401)
        # The device cap rejects redemption without consuming its code.
        code = create()
        for i in range(32):
            sql('insert into paired_devices values(?,?,?,?,?)', (str(i), str(i), int(time.time()), int(time.time())+600, hashlib.sha256(root.encode()).hexdigest()))
        request('pairing/redeem', {'code':code}, token='', expected=400)
        sql('delete from paired_devices')
        credential = request('pairing/redeem', {'code':code}, token='')['token']
        pending = create()
        p.terminate(); p.wait(timeout=5)
        env['XENIA_TOKEN'] = 'synthetic-rotated-controller-token'; p = start()
        request('state', token=credential, expected=401)
        request('pairing/redeem', {'code':pending}, token='', expected=400)
        print('Pairing authorization, one-use codes, replacement, restart, expiry, cancellation, logout, revocation, root rotation: PASS')
    finally:
        p.terminate(); p.wait(timeout=5)
